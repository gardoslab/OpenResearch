//! The few Slack Web API methods the Slack app (0.5.0) needs: posting as
//! the bot (which, unlike a webhook, returns the message's `ts`), reacting,
//! naming a user, reading a thread back, and opening a Socket Mode
//! connection. Tokens come from `config::slack_app()`; nothing here stores
//! them.

use std::time::Duration;

use serde_json::{json, Value};

const DEFAULT_BASE: &str = "https://slack.com/api";

/// How a Web API call failed. Slack answers most mistakes with HTTP 200 and
/// `"ok": false`; the error code decides whether trying again can help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// A revoked token, a missing scope, a channel the bot is not in…
    Rejected(String),
    /// Rate limits, Slack-side outages, network failures.
    Retryable(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(error) | Self::Retryable(error) => f.write_str(error),
        }
    }
}

/// Where a bot-posted message landed: its thread's address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posted {
    pub channel: String,
    pub ts: String,
}

fn http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// Error codes that mean "not now" rather than "not ever".
fn retryable_code(code: &str) -> bool {
    matches!(
        code,
        "ratelimited"
            | "internal_error"
            | "fatal_error"
            | "service_unavailable"
            | "request_timeout"
            | "team_added_to_org"
    )
}

pub struct SlackApi {
    token: String,
    base: String,
}

impl SlackApi {
    pub fn new(token: impl Into<String>) -> Self {
        Self::with_base(token, DEFAULT_BASE)
    }

    /// Against a local stub in tests.
    pub fn with_base(token: impl Into<String>, base: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            base: base.into(),
        }
    }

    /// Write methods take a JSON body; read methods (`users.info`,
    /// `conversations.replies`) only take form arguments, hence `form`.
    async fn call(
        &self,
        method: &str,
        body: Option<&Value>,
        form: &[(&str, &str)],
    ) -> Result<Value, ApiError> {
        let mut request = http()
            .post(format!("{}/{method}", self.base))
            .bearer_auth(&self.token)
            .timeout(Duration::from_secs(10));
        request = match body {
            Some(body) => request.json(body),
            None => request.form(form),
        };
        let response = request
            .send()
            .await
            .map_err(|err| ApiError::Retryable(format!("could not reach Slack: {err}")))?;
        let status = response.status();
        if status.as_u16() == 429 || status.is_server_error() {
            return Err(ApiError::Retryable(format!("Slack answered {status}")));
        }
        if !status.is_success() {
            return Err(ApiError::Rejected(format!("Slack answered {status}")));
        }
        let value: Value = response
            .json()
            .await
            .map_err(|err| ApiError::Retryable(format!("unreadable Slack response: {err}")))?;
        if value["ok"].as_bool() == Some(true) {
            return Ok(value);
        }
        let code = value["error"]
            .as_str()
            .unwrap_or("unknown_error")
            .to_string();
        Err(if retryable_code(&code) {
            ApiError::Retryable(code)
        } else {
            ApiError::Rejected(code)
        })
    }

    /// `chat.postMessage`. `thread_ts` posts into that message's thread.
    pub async fn post_message(
        &self,
        channel: &str,
        text: &str,
        blocks: Option<&Value>,
        thread_ts: Option<&str>,
    ) -> Result<Posted, ApiError> {
        let mut body = json!({
            "channel": channel,
            "text": text,
            "unfurl_links": false,
            "unfurl_media": false,
        });
        if let Some(blocks) = blocks {
            body["blocks"] = blocks.clone();
        }
        if let Some(thread_ts) = thread_ts {
            body["thread_ts"] = json!(thread_ts);
        }
        let value = self.call("chat.postMessage", Some(&body), &[]).await?;
        match (value["channel"].as_str(), value["ts"].as_str()) {
            (Some(channel), Some(ts)) => Ok(Posted {
                channel: channel.to_string(),
                ts: ts.to_string(),
            }),
            _ => Err(ApiError::Rejected(
                "chat.postMessage returned no channel or ts".to_string(),
            )),
        }
    }

    /// `reactions.add`. Reacting twice is not a failure.
    pub async fn add_reaction(&self, channel: &str, ts: &str, name: &str) -> Result<(), ApiError> {
        let body = json!({ "channel": channel, "timestamp": ts, "name": name });
        match self.call("reactions.add", Some(&body), &[]).await {
            Err(ApiError::Rejected(code)) if code == "already_reacted" => Ok(()),
            other => other.map(|_| ()),
        }
    }

    /// `auth.test`: which workspace and bot user the token belongs to.
    pub async fn auth_test(&self) -> Result<Value, ApiError> {
        self.call("auth.test", Some(&json!({})), &[]).await
    }

    /// The name Slack shows for `user_id`.
    pub async fn user_name(&self, user_id: &str) -> Result<String, ApiError> {
        let value = self.call("users.info", None, &[("user", user_id)]).await?;
        let user = &value["user"];
        let name = [
            &user["profile"]["display_name"],
            &user["real_name"],
            &user["name"],
        ]
        .into_iter()
        .filter_map(Value::as_str)
        .find(|name| !name.trim().is_empty())
        .unwrap_or(user_id)
        .to_string();
        Ok(name)
    }

    /// `conversations.replies`: the thread under `ts`, newer than `oldest`
    /// (exclusive), parent included when it is newer. One page is plenty for
    /// the gap a reconnect leaves.
    pub async fn thread_replies(
        &self,
        channel: &str,
        ts: &str,
        oldest: &str,
    ) -> Result<Vec<Value>, ApiError> {
        let value = self
            .call(
                "conversations.replies",
                None,
                &[
                    ("channel", channel),
                    ("ts", ts),
                    ("oldest", oldest),
                    ("limit", "200"),
                ],
            )
            .await?;
        Ok(value["messages"].as_array().cloned().unwrap_or_default())
    }

    /// `apps.connections.open` (app-level token): a single-use WebSocket URL.
    pub async fn open_socket(&self) -> Result<String, ApiError> {
        let value = self.call("apps.connections.open", None, &[]).await?;
        value["url"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| ApiError::Rejected("apps.connections.open returned no url".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::post;

    async fn stub(routes: axum::Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, routes).await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn post_message_returns_where_the_message_landed() {
        let base = stub(axum::Router::new().route(
            "/chat.postMessage",
            post(
                |headers: axum::http::HeaderMap, body: axum::Json<Value>| async move {
                    assert_eq!(headers["authorization"], "Bearer xoxb-test");
                    assert_eq!(body["thread_ts"], "1.000");
                    axum::Json(json!({ "ok": true, "channel": "C1", "ts": "2.000" }))
                },
            ),
        ))
        .await;
        let posted = SlackApi::with_base("xoxb-test", base)
            .post_message("C1", "hi", None, Some("1.000"))
            .await
            .unwrap();
        assert_eq!(
            posted,
            Posted {
                channel: "C1".into(),
                ts: "2.000".into()
            }
        );
    }

    #[tokio::test]
    async fn slack_error_codes_split_into_rejected_and_retryable() {
        let base = stub(
            axum::Router::new()
                .route(
                    "/chat.postMessage",
                    post(|| async {
                        axum::Json(json!({ "ok": false, "error": "not_in_channel" }))
                    }),
                )
                .route(
                    "/auth.test",
                    post(|| async { axum::Json(json!({ "ok": false, "error": "ratelimited" })) }),
                )
                .route(
                    "/reactions.add",
                    post(|| async {
                        axum::Json(json!({ "ok": false, "error": "already_reacted" }))
                    }),
                ),
        )
        .await;
        let api = SlackApi::with_base("xoxb-test", base);
        assert_eq!(
            api.post_message("C1", "hi", None, None).await,
            Err(ApiError::Rejected("not_in_channel".into()))
        );
        assert_eq!(
            api.auth_test().await,
            Err(ApiError::Retryable("ratelimited".into()))
        );
        assert_eq!(api.add_reaction("C1", "1.0", "eyes").await, Ok(()));
    }

    #[tokio::test]
    async fn read_methods_send_form_arguments() {
        let base = stub(axum::Router::new().route(
            "/users.info",
            post(|form: axum::Form<std::collections::HashMap<String, String>>| async move {
                assert_eq!(form["user"], "U1");
                axum::Json(json!({
                    "ok": true,
                    "user": { "name": "jdoe", "real_name": "Jo Doe", "profile": { "display_name": "" } },
                }))
            }),
        ))
        .await;
        let name = SlackApi::with_base("xoxb-test", base)
            .user_name("U1")
            .await
            .unwrap();
        assert_eq!(name, "Jo Doe");
    }
}
