//! Outbound notifications (Slack today, other providers later) queued in
//! `notifications_outbox` and drained by one background loop in `orx up`.
//!
//! Any process holding a `Store` — including a detached `orx supervise` —
//! can call [`crate::store::Store::enqueue_notification`] to drop a message
//! in the queue. Delivery itself only ever happens from the single `orx up`
//! process that owns the drain loop, so two writers never race a send.
//!
//! This module is the drain mechanics and the Slack transport. Deciding
//! *when* to enqueue (a run starting, a run's outcome getting synthesized,
//! a usage limit hit) is each feature's own job — see `notify_events` —
//! as is where a webhook URL comes from (`config_dir()/slack.json`, the
//! same shape as `overleaf.json` in `src/config.rs`). [`spawn_notifier_loop`]
//! is started from `commands::up::run` and re-reads `slack.json` every pass,
//! so saving or removing a webhook or bot token in Settings takes effect
//! without a restart.
//!
//! With a bot token and channel saved, messages go out through
//! `chat.postMessage` instead of the webhook. That call returns the
//! message's `ts`, and a message enqueued for a chat session is recorded in
//! `slack_messages` so a thread reply to it can find the chat again
//! (`slack_inbound`).

use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use crate::slack_api::{ApiError, Posted, SlackApi};
use crate::store::{SlackThread, Store};

/// How one delivery attempt turned out. A rejected payload (a revoked or
/// disabled webhook, a malformed request) is pointless to retry; a network
/// hiccup or a 5xx is worth trying again on the next drain pass.
/// `Sent` carries where the message landed when the provider can say
/// (the bot can, a webhook cannot).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Sent(Option<Posted>),
    Rejected,
    Retryable,
}

/// A place to deliver notifications to. `kind` is the notification's own
/// tag (`"job_submitted"`, `"run_synthesized"`, …); `payload` is whatever
/// the enqueuing code put in `notifications_outbox.payload_json`, parsed.
/// A provider owns its own formatting — this trait does not assume Slack's
/// `text`/`blocks` shape belongs to every future provider.
#[async_trait]
pub trait NotificationProvider: Send + Sync {
    async fn send(&self, kind: &str, payload: &Value) -> DeliveryOutcome;
}

fn http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// Posts to a Slack Incoming Webhook. The payload's `"text"` field is sent
/// verbatim; a caller that wants Slack `blocks` formatting puts them under
/// `"blocks"` in the same payload and this provider forwards them as-is.
pub struct SlackWebhookProvider {
    webhook_url: String,
}

impl SlackWebhookProvider {
    pub fn new(webhook_url: String) -> Self {
        Self { webhook_url }
    }
}

#[async_trait]
impl NotificationProvider for SlackWebhookProvider {
    async fn send(&self, _kind: &str, payload: &Value) -> DeliveryOutcome {
        let mut body = serde_json::Map::new();
        if let Some(text) = payload.get("text") {
            body.insert("text".to_string(), text.clone());
        }
        if let Some(blocks) = payload.get("blocks") {
            body.insert("blocks".to_string(), blocks.clone());
        }
        let Ok(response) = http()
            .post(&self.webhook_url)
            .timeout(Duration::from_secs(5))
            .json(&Value::Object(body))
            .send()
            .await
        else {
            return DeliveryOutcome::Retryable;
        };
        if response.status().is_success() {
            DeliveryOutcome::Sent(None)
        } else if matches!(response.status().as_u16(), 400 | 404 | 410) {
            // Slack answers a revoked/disabled webhook with 404 or 410, and
            // a malformed payload with 400 — none of those improve on retry.
            DeliveryOutcome::Rejected
        } else {
            DeliveryOutcome::Retryable
        }
    }
}

/// Posts as the Slack app's bot. A payload may name its own `channel` and
/// `thread_ts` — a reply into a thread goes wherever that thread is, even
/// after the configured channel changes.
pub struct SlackBotProvider {
    api: SlackApi,
    channel: String,
}

impl SlackBotProvider {
    pub fn new(api: SlackApi, channel: String) -> Self {
        Self { api, channel }
    }
}

#[async_trait]
impl NotificationProvider for SlackBotProvider {
    async fn send(&self, _kind: &str, payload: &Value) -> DeliveryOutcome {
        let channel = payload["channel"].as_str().unwrap_or(&self.channel);
        let text = payload["text"].as_str().unwrap_or_default();
        let blocks = payload.get("blocks").filter(|blocks| blocks.is_array());
        match self
            .api
            .post_message(channel, text, blocks, payload["thread_ts"].as_str())
            .await
        {
            Ok(posted) => DeliveryOutcome::Sent(Some(posted)),
            Err(ApiError::Rejected(_)) => DeliveryOutcome::Rejected,
            Err(ApiError::Retryable(_)) => DeliveryOutcome::Retryable,
        }
    }
}

/// Whichever Slack transport is configured: the bot when it is, else the
/// webhook. A thread reply only makes sense through the bot.
pub struct SlackProvider {
    bot: Option<SlackBotProvider>,
    webhook: Option<SlackWebhookProvider>,
}

impl SlackProvider {
    /// From what `slack.json` holds right now; `None` when nothing can post.
    pub fn from_config() -> Option<Self> {
        let bot = crate::config::slack_app()
            .poster()
            .map(|(token, channel)| SlackBotProvider::new(SlackApi::new(token), channel));
        let webhook = crate::config::slack_webhook_url().map(SlackWebhookProvider::new);
        (bot.is_some() || webhook.is_some()).then_some(Self { bot, webhook })
    }
}

#[async_trait]
impl NotificationProvider for SlackProvider {
    async fn send(&self, kind: &str, payload: &Value) -> DeliveryOutcome {
        match (&self.bot, &self.webhook) {
            (Some(bot), _) => bot.send(kind, payload).await,
            (None, _) if payload.get("thread_ts").is_some() => DeliveryOutcome::Rejected,
            (None, Some(webhook)) => webhook.send(kind, payload).await,
            (None, None) => DeliveryOutcome::Retryable,
        }
    }
}

/// Attempts a notification is allowed before the drain loop gives up on it
/// (it stays in the outbox, unsent, rather than being deleted — visible for
/// debugging, harmless since nothing else reads unsent rows as "pending
/// work" outside this module). Matches "a few attempts with backoff": the
/// backoff is simply the drain loop's own poll interval between attempts.
const MAX_ATTEMPTS: i64 = 3;

/// One drain pass: attempt delivery of every currently-pending notification
/// (up to `MAX_ATTEMPTS` each) through `provider`, oldest first.
///
/// Takes `store` by value, not `&Store`: `rusqlite::Connection` isn't
/// `Sync`, so a `&Store` held across the `provider.send(...).await` below
/// would make this function's future `!Send` — fatal for `tokio::spawn` in
/// [`spawn_notifier_loop`], which needs to hand the task to any worker
/// thread. An owned `Store` has no such borrow and is `Send`. Callers reopen
/// a fresh connection each time (cheap: WAL mode, a short busy timeout)
/// rather than share one across calls.
pub async fn drain_once(
    store: Store,
    provider: &dyn NotificationProvider,
) -> crate::error::Result<()> {
    for notification in store.list_pending_notifications(20)? {
        if notification.attempts >= MAX_ATTEMPTS {
            continue;
        }
        let payload: Value = match serde_json::from_str(&notification.payload_json) {
            Ok(value) => value,
            Err(err) => {
                // Never sendable — record it and let MAX_ATTEMPTS retire the
                // row like any other permanent failure, rather than looping.
                store.mark_notification_attempt_failed(&notification.id, &err.to_string())?;
                continue;
            }
        };
        match provider.send(&notification.kind, &payload).await {
            DeliveryOutcome::Sent(posted) => {
                store.mark_notification_sent(&notification.id)?;
                // Only a thread's parent is recorded: a reply anywhere in the
                // thread carries the parent's ts as its `thread_ts`.
                if let (Some(posted), Some(session_id), None) = (
                    posted,
                    notification.chat_session_id.as_deref(),
                    payload.get("thread_ts"),
                ) {
                    store.record_slack_thread(&SlackThread {
                        channel: posted.channel,
                        ts: posted.ts,
                        chat_session_id: session_id.to_string(),
                        run_id: notification.run_id.clone(),
                        kind: notification.kind.clone(),
                        created_at: crate::store::now_ms(),
                    })?;
                }
            }
            DeliveryOutcome::Rejected => {
                store.mark_notification_attempt_failed(&notification.id, "rejected by provider")?
            }
            DeliveryOutcome::Retryable => store.mark_notification_attempt_failed(
                &notification.id,
                "delivery attempt failed, will retry",
            )?,
        }
    }
    Ok(())
}

/// Drains the outbox on an interval for as long as `orx up` runs, through a
/// Slack provider built from whatever `slack.json` holds at that moment —
/// nothing saved means the pass is skipped and rows wait. Exactly
/// one of these should run per process — a detached `orx supervise` only
/// ever enqueues (`Store::enqueue_notification`), never drains, so no two
/// writers can race a send. Opens a fresh `Store` each tick (cheap: WAL
/// mode, a short busy timeout) rather than holding one connection across
/// every await in between, matching `watch_runs`'s loop.
pub fn spawn_notifier_loop() {
    tokio::spawn(async move {
        loop {
            let provider = SlackProvider::from_config();
            match (provider, Store::open()) {
                (None, _) => {}
                (Some(provider), Ok(store)) => {
                    if let Err(err) = drain_once(store, &provider).await {
                        eprintln!("orx up: notification drain failed (will retry): {err}");
                    }
                }
                (Some(_), Err(err)) => {
                    eprintln!("orx up: notification drain could not open the store: {err}")
                }
            }
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Records every `(kind, payload)` it was asked to send and returns a
    /// scripted outcome per call, so tests can drive multi-attempt behavior
    /// without a real HTTP server.
    struct ScriptedProvider {
        outcomes: Mutex<Vec<DeliveryOutcome>>,
        calls: Mutex<Vec<(String, Value)>>,
    }

    impl ScriptedProvider {
        fn new(outcomes: Vec<DeliveryOutcome>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl NotificationProvider for ScriptedProvider {
        async fn send(&self, kind: &str, payload: &Value) -> DeliveryOutcome {
            self.calls
                .lock()
                .unwrap()
                .push((kind.to_string(), payload.clone()));
            let mut outcomes = self.outcomes.lock().unwrap();
            if outcomes.is_empty() {
                DeliveryOutcome::Sent(None)
            } else {
                outcomes.remove(0)
            }
        }
    }

    /// A fresh connection to the same on-disk database each call — exactly
    /// how `spawn_notifier_loop` uses `drain_once` in production, and
    /// necessary here since `drain_once` now consumes its `Store`.
    fn open(dir: &std::path::Path) -> Store {
        Store::open_at(dir.to_path_buf()).unwrap()
    }

    fn temp_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("orx-notify-{}", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn a_sent_notification_leaves_the_pending_list() {
        let dir = temp_dir();
        open(&dir)
            .enqueue_notification("job_submitted", None, "{\"text\":\"hello\"}")
            .unwrap();
        let provider = ScriptedProvider::new(vec![]);

        drain_once(open(&dir), &provider).await.unwrap();

        assert_eq!(open(&dir).list_pending_notifications(10).unwrap().len(), 0);
        assert_eq!(
            provider.calls.lock().unwrap().as_slice(),
            [(
                "job_submitted".to_string(),
                serde_json::json!({"text": "hello"})
            )]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_bot_posted_session_message_is_recorded_as_a_thread() {
        let dir = temp_dir();
        let store = open(&dir);
        store
            .enqueue_session_notification(
                "run_synthesized",
                Some("run_1"),
                Some("chat_A"),
                "{\"text\":\"done\"}",
            )
            .unwrap();
        store
            .enqueue_session_notification(
                "slack_reply",
                None,
                Some("chat_A"),
                "{\"text\":\"answer\",\"thread_ts\":\"1.0\"}",
            )
            .unwrap();
        store
            .enqueue_notification("digest_daily", None, "{\"text\":\"digest\"}")
            .unwrap();
        drop(store);
        let posted = |ts: &str| {
            DeliveryOutcome::Sent(Some(Posted {
                channel: "C1".into(),
                ts: ts.into(),
            }))
        };
        let provider = ScriptedProvider::new(vec![posted("1.0"), posted("2.0"), posted("3.0")]);

        drain_once(open(&dir), &provider).await.unwrap();

        let store = open(&dir);
        let thread = store.slack_thread("C1", "1.0").unwrap().unwrap();
        assert_eq!(thread.chat_session_id, "chat_A");
        assert_eq!(thread.run_id.as_deref(), Some("run_1"));
        // Neither the reply inside a thread nor a session-less digest is one.
        assert_eq!(store.slack_thread("C1", "2.0").unwrap(), None);
        assert_eq!(store.slack_thread("C1", "3.0").unwrap(), None);
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_retryable_failure_stays_pending_for_the_next_drain_pass() {
        let dir = temp_dir();
        let id = open(&dir)
            .enqueue_notification("run_synthesized", None, "{\"text\":\"done\"}")
            .unwrap();
        let provider = ScriptedProvider::new(vec![DeliveryOutcome::Retryable]);

        drain_once(open(&dir), &provider).await.unwrap();
        let pending = open(&dir).list_pending_notifications(10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id);
        assert_eq!(pending[0].attempts, 1);

        // A second pass succeeds and the row finally clears.
        drain_once(open(&dir), &provider).await.unwrap();
        assert_eq!(open(&dir).list_pending_notifications(10).unwrap().len(), 0);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_notification_stops_being_attempted_after_max_attempts() {
        let dir = temp_dir();
        open(&dir)
            .enqueue_notification("job_submitted", None, "{\"text\":\"x\"}")
            .unwrap();
        let provider = ScriptedProvider::new(vec![
            DeliveryOutcome::Retryable,
            DeliveryOutcome::Retryable,
            DeliveryOutcome::Retryable,
            DeliveryOutcome::Retryable,
        ]);

        for _ in 0..(MAX_ATTEMPTS + 2) {
            drain_once(open(&dir), &provider).await.unwrap();
        }

        // Exactly MAX_ATTEMPTS calls reached the provider, not MAX_ATTEMPTS + 2.
        assert_eq!(provider.calls.lock().unwrap().len(), MAX_ATTEMPTS as usize);
        // Still sitting in the outbox, unsent — visible, not silently dropped.
        assert_eq!(open(&dir).list_pending_notifications(10).unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn an_unparseable_payload_is_recorded_and_not_retried_forever() {
        let dir = temp_dir();
        open(&dir)
            .enqueue_notification("job_submitted", None, "not json")
            .unwrap();
        let provider = ScriptedProvider::new(vec![]);

        for _ in 0..(MAX_ATTEMPTS + 1) {
            drain_once(open(&dir), &provider).await.unwrap();
        }

        // The provider was never called — the payload never parsed.
        assert_eq!(provider.calls.lock().unwrap().len(), 0);
        assert_eq!(open(&dir).list_pending_notifications(10).unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(dir);
    }

    /// `SlackWebhookProvider` against a real local HTTP server — never a live
    /// `hooks.slack.com` URL, per this track's credential-handling rule.
    /// Doubles as coverage for the `/api/settings/slack/preflight` route,
    /// which sends through the exact same provider.
    #[tokio::test]
    async fn slack_webhook_provider_delivers_through_a_local_axum_stub() {
        use axum::extract::State;
        use axum::http::StatusCode;
        use axum::routing::post;
        use std::sync::atomic::{AtomicU16, Ordering};

        async fn respond(
            State(status): State<Arc<AtomicU16>>,
            body: axum::body::Bytes,
        ) -> StatusCode {
            let value: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(value["text"], "hello from orx");
            StatusCode::from_u16(status.load(Ordering::SeqCst)).unwrap()
        }

        let status = Arc::new(AtomicU16::new(200));
        let app = axum::Router::new()
            .route("/webhook", post(respond))
            .with_state(status.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let provider = SlackWebhookProvider::new(format!("http://{addr}/webhook"));
        let message = serde_json::json!({ "text": "hello from orx" });

        status.store(200, Ordering::SeqCst);
        assert_eq!(
            provider.send("preflight", &message).await,
            DeliveryOutcome::Sent(None)
        );

        status.store(404, Ordering::SeqCst);
        assert_eq!(
            provider.send("preflight", &message).await,
            DeliveryOutcome::Rejected
        );

        status.store(500, Ordering::SeqCst);
        assert_eq!(
            provider.send("preflight", &message).await,
            DeliveryOutcome::Retryable
        );
    }
}
