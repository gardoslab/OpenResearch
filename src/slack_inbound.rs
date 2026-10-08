//! Slack thread replies coming back in (0.5.0), over Socket Mode.
//!
//! One `orx up` per Slack app holds the connection (`SlackApp::hold_socket`):
//! Slack hands each event to one of the open connections at random, so a
//! second holder would silently take half the replies. The connection is
//! outbound (`apps.connections.open`, then a WebSocket), so no public URL is
//! needed from a laptop or an SCC node.
//!
//! Intake is deliberately narrow. A reply is taken in only when it is in a
//! thread the bot started about a chat (`slack_messages`), comes from an
//! allow-listed Slack user (the chat's project's own list when it has one,
//! else the global one), and is not a bot's own message: a reply lands in
//! an agent with a shell, so everything else is dropped. What passes goes to
//! `slack_inbox` *before* the envelope is acked, so a crash after the ack
//! cannot lose it, and `(channel, ts)` being unique there turns Slack's
//! retries into no-ops. Delivery into the chat is `local::chat`'s job
//! (`process_slack_inbox`), from the same loop that delivers run wake-ups.
//!
//! Replies sent while no connection was open are fetched on reconnect with
//! `conversations.replies`, for threads from the last week.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

use crate::config::SlackApp;
use crate::slack_api::{ApiError, SlackApi};
use crate::store::Store;

/// Threads older than this are not backfilled; a reply that late is rare,
/// and each thread costs one API call per reconnect.
const BACKFILL_WINDOW: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const BACKFILL_MAX_THREADS: usize = 50;
/// Slack pings every few seconds; this much silence means the socket is dead.
const SILENCE_LIMIT: Duration = Duration::from_secs(120);
const MAX_BACKOFF: Duration = Duration::from_secs(300);

// --- status -------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct Status {
    state: &'static str,
    since: i64,
    error: Option<String>,
}

fn status() -> &'static Mutex<Status> {
    static STATUS: std::sync::OnceLock<Mutex<Status>> = std::sync::OnceLock::new();
    STATUS.get_or_init(|| {
        Mutex::new(Status {
            state: "off",
            ..Status::default()
        })
    })
}

fn set_status(state: &'static str, error: Option<String>) {
    let mut status = status().lock().unwrap_or_else(|e| e.into_inner());
    if status.state != state || status.error != error {
        *status = Status {
            state,
            since: crate::store::now_ms(),
            error,
        };
    }
}

/// For Settings → Slack: `off`, `connecting`, `connected` or `error`.
pub fn socket_status_json() -> Value {
    let status = status().lock().unwrap_or_else(|e| e.into_inner()).clone();
    json!({ "state": status.state, "since": status.since, "error": status.error })
}

// --- intake -------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum Intake {
    Accepted,
    Duplicate,
    Ignored(&'static str),
}

/// Take in one `message` event (live, or from a backfill with `channel`
/// filled in) if it is a reply we should deliver. See the module doc.
/// `global_allowed_user_ids` applies unless the thread's project has its own.
pub fn intake(
    store: &Store,
    global_allowed_user_ids: &[String],
    event: &Value,
) -> crate::error::Result<Intake> {
    if event["type"].as_str() != Some("message") {
        return Ok(Intake::Ignored("not a message"));
    }
    // Edits, deletions, joins and bot posts all carry a subtype; a reply also
    // sent to the channel is still a plain reply.
    if event["subtype"]
        .as_str()
        .is_some_and(|subtype| subtype != "thread_broadcast")
    {
        return Ok(Intake::Ignored("not a plain message"));
    }
    if event.get("bot_id").is_some_and(|id| !id.is_null()) {
        return Ok(Intake::Ignored("from a bot"));
    }
    let (Some(channel), Some(ts), Some(thread_ts), Some(user)) = (
        event["channel"].as_str(),
        event["ts"].as_str(),
        event["thread_ts"].as_str(),
        event["user"].as_str(),
    ) else {
        return Ok(Intake::Ignored("not a thread reply"));
    };
    if ts == thread_ts {
        return Ok(Intake::Ignored("a thread parent"));
    }
    let Some(thread) = store.slack_thread(channel, thread_ts)? else {
        return Ok(Intake::Ignored("not a thread orx started"));
    };
    let project_allowed = match store.get_chat_session(&thread.chat_session_id)? {
        Some(session) => store.project_slack(&session.project_id)?.allowed_user_ids,
        None => None,
    };
    let allowed = project_allowed
        .as_deref()
        .unwrap_or(global_allowed_user_ids);
    if !allowed.iter().any(|id| id == user) {
        return Ok(Intake::Ignored("sender not on the allow-list"));
    }
    let text = event["text"].as_str().unwrap_or_default().trim();
    if text.is_empty() {
        return Ok(Intake::Ignored("empty"));
    }
    Ok(
        if store.insert_slack_inbox(channel, ts, thread_ts, user, text, &thread.chat_session_id)? {
            Intake::Accepted
        } else {
            Intake::Duplicate
        },
    )
}

/// Slack escapes these three in message text; the agent should see the
/// characters the person typed.
pub fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The name to show for a Slack user, asked once per process.
pub async fn user_name(user_id: &str) -> String {
    static NAMES: std::sync::OnceLock<Mutex<HashMap<String, String>>> = std::sync::OnceLock::new();
    let names = NAMES.get_or_init(Default::default);
    if let Some(name) = names.lock().unwrap_or_else(|e| e.into_inner()).get(user_id) {
        return name.clone();
    }
    let Some((token, _)) = crate::config::slack_app().poster() else {
        return user_id.to_string();
    };
    match SlackApi::new(token).user_name(user_id).await {
        Ok(name) => {
            names
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(user_id.to_string(), name.clone());
            name
        }
        Err(_) => user_id.to_string(),
    }
}

// --- backfill -----------------------------------------------------------------

/// Fetch the replies each recent thread got while no socket was open.
async fn backfill(app: &SlackApp) {
    let Some((token, _)) = app.poster() else {
        return;
    };
    let since = crate::store::now_ms() - BACKFILL_WINDOW.as_millis() as i64;
    let threads = match Store::open()
        .and_then(|store| store.recent_slack_threads(since, BACKFILL_MAX_THREADS))
    {
        Ok(threads) => threads,
        Err(err) => {
            eprintln!("orx up: Slack backfill could not read threads: {err}");
            return;
        }
    };
    let api = SlackApi::new(token);
    let mut accepted = 0;
    for (thread, newest_reply) in threads {
        let oldest = newest_reply.unwrap_or_else(|| thread.ts.clone());
        let messages = match api
            .thread_replies(&thread.channel, &thread.ts, &oldest)
            .await
        {
            Ok(messages) => messages,
            Err(ApiError::Rejected(code)) => {
                // The bot left the channel or the thread is gone; the others
                // may still be fine.
                eprintln!("orx up: Slack backfill skipped a thread: {code}");
                continue;
            }
            Err(ApiError::Retryable(error)) => {
                eprintln!("orx up: Slack backfill stopped early: {error}");
                break;
            }
        };
        let Ok(store) = Store::open() else { return };
        for mut message in messages {
            message["channel"] = json!(thread.channel);
            if let Ok(Intake::Accepted) = intake(&store, &app.allowed_user_ids, &message) {
                accepted += 1;
            }
        }
        // Keeps a week of threads well inside `conversations.replies`' rate limit.
        tokio::time::sleep(Duration::from_millis(1200)).await;
    }
    if accepted > 0 {
        eprintln!("orx up: took in {accepted} Slack repl(ies) sent while disconnected");
    }
}

// --- socket -------------------------------------------------------------------

enum Ended {
    /// Slack asked us to reconnect, or the settings changed: go again now.
    Reconnect,
    /// The connection failed; wait before the next try.
    Failed(String),
    /// A token was refused; retrying the same one cannot help.
    Refused(String),
}

/// Serve one Socket Mode connection until it ends.
async fn run_connection(app: &SlackApp) -> Ended {
    let url = match SlackApi::new(&app.app_token).open_socket().await {
        Ok(url) => url,
        Err(ApiError::Rejected(code)) => {
            return Ended::Refused(format!("Slack refused the app-level token ({code})"))
        }
        Err(ApiError::Retryable(error)) => return Ended::Failed(error),
    };
    let socket = match tokio_tungstenite::connect_async(url.as_str()).await {
        Ok((socket, _)) => socket,
        Err(err) => {
            return Ended::Failed(format!("could not open the Socket Mode connection: {err}"))
        }
    };
    let (mut sink, mut stream) = socket.split();
    let mut settings_check = tokio::time::interval(Duration::from_secs(15));
    settings_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_heard = Instant::now();
    loop {
        tokio::select! {
            message = stream.next() => {
                last_heard = Instant::now();
                let text = match message {
                    None | Some(Ok(Message::Close(_))) => return Ended::Reconnect,
                    Some(Err(err)) => return Ended::Failed(format!("Socket Mode connection dropped: {err}")),
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(_)) => continue,
                };
                let Ok(envelope) = serde_json::from_str::<Value>(&text) else { continue };
                match envelope["type"].as_str() {
                    Some("hello") => {
                        // More than one means another orx (or anything else
                        // holding this app's token) is splitting the events.
                        eprintln!(
                            "orx up: Slack Socket Mode connected ({} open connection(s) for this app)",
                            envelope["num_connections"].as_u64().unwrap_or(1)
                        );
                        set_status("connected", None);
                        let app = app.clone();
                        tokio::spawn(async move { backfill(&app).await });
                    }
                    Some("disconnect") => return Ended::Reconnect,
                    _ => {}
                }
                let Some(envelope_id) = envelope["envelope_id"].as_str() else { continue };
                if envelope["type"].as_str() == Some("events_api") {
                    let event = &envelope["payload"]["event"];
                    let taken = Store::open().and_then(|store| intake(&store, &app.allowed_user_ids, event));
                    match taken {
                        Ok(intake) => eprintln!(
                            "orx up: Slack event {} in {}: {intake:?}",
                            event["type"].as_str().unwrap_or("?"),
                            event["channel"].as_str().unwrap_or("?"),
                        ),
                        Err(err) => {
                            // No ack: Slack retries the event, and the retry
                            // may find the store writable again.
                            eprintln!("orx up: could not take in a Slack event: {err}");
                            continue;
                        }
                    }
                } else {
                    eprintln!(
                        "orx up: Slack envelope {} acknowledged without action",
                        envelope["type"].as_str().unwrap_or("?")
                    );
                }
                let ack = json!({ "envelope_id": envelope_id }).to_string();
                if sink.send(Message::Text(ack.into())).await.is_err() {
                    return Ended::Failed("could not acknowledge a Slack event".to_string());
                }
            }
            _ = settings_check.tick() => {
                if last_heard.elapsed() > SILENCE_LIMIT {
                    return Ended::Failed("Slack went quiet".to_string());
                }
                let current = crate::config::slack_app();
                if !current.socket_ready() || current != *app {
                    let _ = sink.send(Message::Close(None)).await;
                    return Ended::Reconnect;
                }
            }
        }
    }
}

/// Hold the Socket Mode connection for as long as `orx up` runs, whenever
/// Settings says this machine should. Re-reads `slack.json` between
/// connections (and every few seconds during one), so turning it on or off
/// needs no restart.
pub fn spawn_socket_loop() {
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(1);
        let mut refused: Option<SlackApp> = None;
        loop {
            let app = crate::config::slack_app();
            if !app.socket_ready() {
                set_status("off", None);
                refused = None;
                tokio::time::sleep(Duration::from_secs(10)).await;
                continue;
            }
            // Wait for new tokens rather than hammering Slack with a refused one.
            if refused.as_ref() == Some(&app) {
                tokio::time::sleep(Duration::from_secs(10)).await;
                continue;
            }
            set_status("connecting", None);
            let started = Instant::now();
            match run_connection(&app).await {
                Ended::Reconnect => {
                    backoff = Duration::from_secs(1);
                    // A floor, so a server that closes at once is not spun on.
                    tokio::time::sleep(backoff).await;
                    continue;
                }
                Ended::Refused(error) => {
                    eprintln!("orx up: Slack: {error}");
                    set_status("error", Some(error));
                    refused = Some(app);
                    continue;
                }
                Ended::Failed(error) => {
                    set_status("error", Some(error));
                    if started.elapsed() > Duration::from_secs(60) {
                        backoff = Duration::from_secs(1);
                    }
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SlackThread;

    fn store() -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("orx-slack-inbound-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .record_slack_thread(&SlackThread {
                channel: "C1".into(),
                ts: "100.0".into(),
                chat_session_id: "chat_A".into(),
                run_id: Some("run_1".into()),
                kind: "run_synthesized".into(),
                created_at: crate::store::now_ms(),
            })
            .unwrap();
        (store, dir)
    }

    fn reply(user: &str, ts: &str) -> Value {
        json!({
            "type": "message",
            "channel": "C1",
            "user": user,
            "ts": ts,
            "thread_ts": "100.0",
            "text": "try lr=3e-4 &amp; rerun",
        })
    }

    #[test]
    fn an_allow_listed_reply_in_a_known_thread_is_taken_in_once() {
        let (store, dir) = store();
        let allowed = vec!["U1".to_string()];
        assert_eq!(
            intake(&store, &allowed, &reply("U1", "101.0")).unwrap(),
            Intake::Accepted
        );
        // Slack's retry of the same event, or the backfill seeing it again.
        assert_eq!(
            intake(&store, &allowed, &reply("U1", "101.0")).unwrap(),
            Intake::Duplicate
        );
        let pending = store.list_pending_slack_inbox(10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].chat_session_id, "chat_A");
        assert_eq!(unescape(&pending[0].text), "try lr=3e-4 & rerun");
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn everything_else_is_dropped() {
        let (store, dir) = store();
        let allowed = vec!["U1".to_string()];
        let mut from_bot = reply("U1", "102.0");
        from_bot["bot_id"] = json!("B1");
        let mut edited = reply("U1", "103.0");
        edited["subtype"] = json!("message_changed");
        let mut other_thread = reply("U1", "104.0");
        other_thread["thread_ts"] = json!("99.0");
        let mut top_level = reply("U1", "105.0");
        top_level.as_object_mut().unwrap().remove("thread_ts");
        let mut parent = reply("U1", "100.0");
        parent["ts"] = json!("100.0");

        for (event, why) in [
            (reply("U2", "101.0"), "sender not on the allow-list"),
            (from_bot, "from a bot"),
            (edited, "not a plain message"),
            (other_thread, "not a thread orx started"),
            (top_level, "not a thread reply"),
            (parent, "a thread parent"),
        ] {
            assert_eq!(
                intake(&store, &allowed, &event).unwrap(),
                Intake::Ignored(why)
            );
        }
        assert!(store.list_pending_slack_inbox(10).unwrap().is_empty());
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_reply_also_sent_to_the_channel_still_counts() {
        let (store, dir) = store();
        let mut broadcast = reply("U1", "106.0");
        broadcast["subtype"] = json!("thread_broadcast");
        assert_eq!(
            intake(&store, &["U1".to_string()], &broadcast).unwrap(),
            Intake::Accepted
        );
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_project_allow_list_replaces_the_global_one() {
        let (store, dir) = store();
        store
            .create_chat_session(&crate::store::StoredChatSession {
                id: "chat_A".into(),
                project_id: "proj_1".into(),
                harness: "codex".into(),
                native_session_id: None,
                title: None,
                title_source: None,
                model: None,
                service_tier: None,
                permission_mode: None,
                plan_mode: false,
                plan_reset_pending: false,
                reasoning_level: None,
                archived: false,
                context_usage_json: None,
                bootstrap_context: None,
                goal: None,
                active_leaf_id: None,
                parent_session_id: None,
                created_at: 1,
                updated_at: 1,
            })
            .unwrap();
        store
            .set_project_slack(
                "proj_1",
                &crate::store::ProjectSlack {
                    allowed_user_ids: Some(vec!["U2".into()]),
                    ..Default::default()
                },
            )
            .unwrap();
        let global = vec!["U1".to_string()];
        assert_eq!(
            intake(&store, &global, &reply("U1", "101.0")).unwrap(),
            Intake::Ignored("sender not on the allow-list")
        );
        assert_eq!(
            intake(&store, &global, &reply("U2", "102.0")).unwrap(),
            Intake::Accepted
        );
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_backfill_resumes_after_the_newest_reply_taken_in() {
        let (store, dir) = store();
        let allowed = vec!["U1".to_string()];
        intake(&store, &allowed, &reply("U1", "101.0")).unwrap();
        intake(&store, &allowed, &reply("U1", "110.0")).unwrap();
        let threads = store.recent_slack_threads(0, 10).unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].1.as_deref(), Some("110.0"));
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}
