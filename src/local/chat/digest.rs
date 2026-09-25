//! Slack digests: one per project, weekday mornings.
//!
//! - **Daily** (Tue–Fri, from `DIGEST_HOUR`): runs in flight plus the next
//!   steps this week's chats left open. One tool-free `one_shot` call — the
//!   facts are all in the store, so there is nothing for an agent to look up.
//! - **Weekly** (Monday, from `DIGEST_HOUR`): last week's progress, runs in
//!   flight, open next steps, and a roundup of relevant new work. That last
//!   part needs literature and web search, so it runs as a real agent turn in
//!   its own chat session, which also leaves the roundup readable in orx.
//!   Monday gets the weekly only: it already covers everything the daily
//!   would, and two posts in the same minute is the noise we're avoiding.
//!
//! Each `(project, kind, period)` is a row in `slack_digests`, inserted as the
//! claim, so a restart or a second tick never posts twice. A missed 9am
//! (laptop asleep, `orx up` down) posts once on the next tick the same day;
//! a day that passes entirely is simply skipped rather than posted late.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use jiff::{civil::Weekday, Zoned};

use super::{
    closing_outcome, final_answer_text, truncated, ChatHost, SpawnOutcome, TurnGuard,
    TurnSubmission, WirePart,
};
use crate::error::Result;
use crate::jobs::BackendDescriptor;
use crate::local::harness::{OneShot, OneShotQuality, PermissionMode};
use crate::local::model::LocalProject;
use crate::store::{now_ms, Store, StoredChatSession};

/// Local hour (24h) from which the day's digest is due.
const DIGEST_HOUR: i8 = 9;
const TICK: Duration = Duration::from_secs(60);
/// A daily one-shot that takes longer than this has hung.
const DAILY_TIMEOUT: Duration = Duration::from_secs(4 * 60);
/// A weekly agent turn still busy after this is stuck (most likely on a
/// permission prompt nobody is watching); it is stopped and dropped.
const WEEKLY_TIMEOUT_MS: i64 = 2 * 60 * 60 * 1000;
const MAX_ATTEMPTS: i64 = 3;

/// Transcript budget handed to the model: per message, per chat, overall.
const MESSAGE_CHARS: usize = 600;
const SESSION_CHARS: usize = 3000;
const CONTEXT_CHARS: usize = 20_000;
const MAX_SESSIONS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DigestKind {
    Daily,
    Weekly,
}

impl DigestKind {
    fn as_str(self) -> &'static str {
        match self {
            DigestKind::Daily => "daily",
            DigestKind::Weekly => "weekly",
        }
    }

    fn notification_kind(self) -> &'static str {
        match self {
            DigestKind::Daily => "digest_daily",
            DigestKind::Weekly => "digest_weekly",
        }
    }

    fn enabled(self) -> bool {
        let events = crate::telemetry::slack_event_settings();
        let on = match self {
            DigestKind::Daily => events.daily_digest,
            DigestKind::Weekly => events.weekly_digest,
        };
        on && crate::config::slack_webhook_url().is_some()
    }
}

/// The digest due right now, if any, and the stretch of time it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Due {
    kind: DigestKind,
    /// `YYYY-MM-DD`: the day for a daily, that week's Monday for a weekly.
    period: String,
    /// Slack header after the project name.
    title: String,
    /// Chats and finished runs are read from `[window_start, window_end)`.
    window_start_ms: i64,
    window_end_ms: i64,
}

/// `weekly` is whether the weekly digest is on: with it off, Monday falls back
/// to an ordinary daily rather than posting nothing.
fn due_digest(now: &Zoned, weekly: bool) -> Option<Due> {
    let weekday = now.weekday();
    if matches!(weekday, Weekday::Saturday | Weekday::Sunday) || now.hour() < DIGEST_HOUR {
        return None;
    }
    let today = now.date();
    let monday = today
        .checked_sub(jiff::Span::new().days(weekday.to_monday_zero_offset()))
        .ok()?;
    let midnight = |date: jiff::civil::Date| {
        date.to_zoned(now.time_zone().clone())
            .ok()
            .map(|z| z.timestamp().as_millisecond())
    };
    if weekday == Weekday::Monday && weekly {
        let last_monday = monday.checked_sub(jiff::Span::new().days(7)).ok()?;
        return Some(Due {
            kind: DigestKind::Weekly,
            period: monday.to_string(),
            title: format!(
                "Weekly digest \u{b7} week of {}",
                last_monday.strftime("%b %-d")
            ),
            window_start_ms: midnight(last_monday)?,
            window_end_ms: midnight(monday)?,
        });
    }
    Some(Due {
        kind: DigestKind::Daily,
        period: today.to_string(),
        title: format!("Daily digest \u{b7} {}", today.strftime("%a %b %-d")),
        window_start_ms: midnight(monday)?,
        window_end_ms: now.timestamp().as_millisecond(),
    })
}

/// Everything a digest is written from, already rendered as plain text.
#[derive(Debug, Default)]
struct DigestContext {
    active_runs: Vec<String>,
    finished_runs: Vec<String>,
    conversations: Vec<String>,
}

impl DigestContext {
    fn is_empty(&self) -> bool {
        self.active_runs.is_empty()
            && self.finished_runs.is_empty()
            && self.conversations.is_empty()
    }

    fn render(&self) -> String {
        let section = |title: &str, items: &[String]| {
            if items.is_empty() {
                format!("## {title}\n(none)\n")
            } else {
                format!("## {title}\n{}\n", items.join("\n"))
            }
        };
        format!(
            "{}\n{}\n{}",
            section("Runs in flight", &self.active_runs),
            section("Runs finished in this period", &self.finished_runs),
            section("Conversations in this period", &self.conversations),
        )
    }
}

fn hours_since(ms: i64) -> String {
    let hours = (now_ms() - ms).max(0) / 3_600_000;
    if hours < 1 {
        "under an hour".to_string()
    } else {
        format!("{hours}h")
    }
}

fn gather(
    store: &Store,
    project: &LocalProject,
    due: &Due,
    digest_sessions: &HashSet<String>,
) -> Result<DigestContext> {
    let mut context = DigestContext::default();
    for run in store.list_runs_by_project(&project.id)? {
        let experiment = store
            .get_local_experiment(&run.experiment_id)?
            .map(|e| e.display_name().to_string())
            .unwrap_or_else(|| run.experiment_id.clone());
        let job = BackendDescriptor::parse(&run.backend_json)
            .ok()
            .and_then(|d| d.job_label())
            .map(|label| format!(" \u{b7} {label}"))
            .unwrap_or_default();
        match run.status.as_str() {
            "starting" | "running" => context.active_runs.push(format!(
                "- {experiment}{job} \u{b7} {} for {}",
                run.status,
                hours_since(run.created_at)
            )),
            _ => {
                let ended = run.ended_at.unwrap_or(run.updated_at);
                if (due.window_start_ms..due.window_end_ms).contains(&ended) {
                    let result = run
                        .result_markdown
                        .as_deref()
                        .map(str::trim)
                        .filter(|r| !r.is_empty())
                        .map(|r| format!(": {}", truncated(r, 300)))
                        .unwrap_or_default();
                    context
                        .finished_runs
                        .push(format!("- {experiment}{job} \u{b7} {}{result}", run.status));
                }
            }
        }
    }

    let mut budget = CONTEXT_CHARS;
    let sessions = store
        .list_chat_sessions_by_project(&project.id)?
        .into_iter()
        .filter(|s| s.updated_at >= due.window_start_ms && !digest_sessions.contains(&s.id))
        .take(MAX_SESSIONS);
    for session in sessions {
        let mut lines = Vec::new();
        for message in store.list_chat_messages(&session.id)? {
            if !(due.window_start_ms..due.window_end_ms).contains(&message.created_at) {
                continue;
            }
            let Ok(parts) = serde_json::from_str::<Vec<WirePart>>(&message.parts_json) else {
                continue;
            };
            let (who, text) = if message.role == "user" {
                let text = parts
                    .iter()
                    .filter(|p| p.kind == "text")
                    .filter_map(|p| p.text.as_deref())
                    .collect::<Vec<_>>()
                    .join("\n");
                ("User", text)
            } else {
                ("Agent", final_answer_text(&parts))
            };
            let text = text.trim();
            if !text.is_empty() {
                lines.push(format!("{who}: {}", truncated(text, MESSAGE_CHARS)));
            }
        }
        if lines.is_empty() {
            continue;
        }
        let title = session.title.as_deref().unwrap_or("Untitled chat");
        let block = truncated(&format!("### {title}\n{}", lines.join("\n")), SESSION_CHARS);
        if block.len() > budget {
            break;
        }
        budget -= block.len();
        context.conversations.push(block);
    }
    Ok(context)
}

const SLACK_STYLE: &str = "Write Slack mrkdwn, not Markdown: *bold* (single asterisks), \
    `code`, bullets as \"• \", links as <https://url|title>. No headings (#), no tables. \
    Plain, direct sentences; no greeting, preamble, sign-off, or filler.";

const DAILY_SYSTEM: &str = "You write a short morning digest for a research project's \
    Slack channel. You are given its runs in flight and this week's conversations with its \
    research agents. Report only what is in the material; never invent results.";

fn daily_prompt(project: &LocalProject, context: &DigestContext) -> String {
    format!(
        "Project: {name}\n\n{material}\n\
         Write the digest with at most two sections:\n\
         *Running now* — one line per run in flight: what it is and how long it has run. \
         Omit the section if nothing is running.\n\
         *Next steps* — the open next steps from these conversations: things proposed, \
         planned, or asked for that the conversations don't show as done. One line each, \
         most important first, at most 6. Omit the section if there are none.\n\
         Keep the whole digest under 12 lines. {SLACK_STYLE} \
         Reply with the digest only.",
        name = project.name,
        material = context.render(),
    )
}

fn weekly_prompt(project: &LocalProject, due: &Due, context: &DigestContext) -> String {
    format!(
        "[orx] Write this week's Slack digest for the project \"{name}\" ({title}). \
         Your final message is posted to Slack verbatim, so it must contain only the digest.\n\n\
         Material from orx for last week:\n\n{material}\n\
         Sections, in order, each omitted if empty:\n\
         *Last week* — what was tried and what came of it, from the runs and conversations \
         above. 2–5 lines.\n\
         *Running now* — one line per run in flight.\n\
         *Next steps* — open next steps the conversations proposed and don't show as done. \
         At most 6, most important first.\n\
         *New developments* — search for work from roughly the last two weeks that bears on \
         this project's topic (read the repository's README, linked paper, and the material \
         above to pin the topic down). Use `orx discover` (e.g. `orx discover embedding \
         \"<topic>\"`) and web search. 3–5 items, one line each: title as a link, then why it \
         matters here. If nothing relevant turned up, say so in one line.\n\n\
         This is read-only research: do not edit files, launch runs, create experiments, or \
         commit. {SLACK_STYLE}",
        name = project.name,
        title = due.title,
        material = context.render(),
    )
}

/// Runs every minute for as long as `orx up` does. Started only when a Slack
/// webhook is configured (same as the notifier loop); each digest kind's own
/// toggle is re-read every tick, so switching one off takes effect live.
pub async fn watch_digests(
    chat: Arc<ChatHost>,
    data_dir_move_in_progress: Arc<std::sync::atomic::AtomicBool>,
) {
    // A daily digest is written in this process, so one left `running` by a
    // previous `orx up` died with it and is safe to retry.
    if let Ok(store) = Store::open() {
        if let Ok(rows) = store.list_digests_in_state(DigestKind::Daily.as_str(), "running") {
            for row in rows {
                let _ = store.set_digest_state(&row, "running", "pending");
            }
        }
    }
    loop {
        tokio::time::sleep(TICK).await;
        if data_dir_move_in_progress.load(std::sync::atomic::Ordering::SeqCst) {
            continue;
        }
        if let Err(err) = tick(&chat, &Zoned::now()).await {
            eprintln!("orx up: digest watcher: {err}");
        }
    }
}

async fn tick(chat: &Arc<ChatHost>, now: &Zoned) -> Result<()> {
    let due = due_digest(now, DigestKind::Weekly.enabled());
    if let Some(due) = due.filter(|due| due.kind.enabled()) {
        let projects = Store::open()?.list_local_projects()?;
        for project in projects {
            match due.kind {
                DigestKind::Daily => daily(&project, &due).await?,
                DigestKind::Weekly => start_weekly(&project, &due).await?,
            }
        }
    }
    advance_weekly(chat).await
}

async fn daily(project: &LocalProject, due: &Due) -> Result<()> {
    let kind = DigestKind::Daily.as_str();
    let (row, context) = {
        let store = Store::open()?;
        let row = match store.get_digest(&project.id, kind, &due.period)? {
            None => None,
            Some(row) if row.state == "pending" => Some(row),
            Some(_) => return Ok(()),
        };
        let context = gather(&store, project, due, &store.digest_session_ids()?)?;
        let row = match row {
            Some(row) => {
                if !store.set_digest_state(&row, "pending", "running")? {
                    return Ok(());
                }
                row
            }
            None => {
                let state = if context.is_empty() {
                    "skipped"
                } else {
                    "running"
                };
                if !store.claim_digest(&project.id, kind, &due.period, state, None)?
                    || context.is_empty()
                {
                    return Ok(());
                }
                store
                    .get_digest(&project.id, kind, &due.period)?
                    .ok_or_else(|| crate::error::anyhow!("digest row vanished"))?
            }
        };
        (row, context)
    };

    let text = match crate::local::starter::resolve_agent().await {
        Some(agent) => match crate::local::harness::chat_harness(&agent.harness) {
            Some(harness) => {
                harness
                    .one_shot(OneShot {
                        system: DAILY_SYSTEM,
                        prompt: &daily_prompt(project, &context),
                        quality: OneShotQuality::Standard,
                        model: agent.effective_model(),
                        timeout: DAILY_TIMEOUT,
                    })
                    .await
            }
            None => None,
        },
        None => None,
    };

    let store = Store::open()?;
    match text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        Some(text) => {
            crate::notify_events::enqueue_digest(
                &store,
                project,
                DigestKind::Daily.notification_kind(),
                &due.title,
                &text,
            )?;
            store.set_digest_state(&row, "running", "done")?;
        }
        None => {
            let attempts = store.record_digest_attempt(&row)?;
            let next = if attempts >= MAX_ATTEMPTS {
                "failed"
            } else {
                "pending"
            };
            store.set_digest_state(&row, "running", next)?;
            eprintln!(
                "orx up: daily digest for {} failed (attempt {attempts})",
                project.name
            );
        }
    }
    Ok(())
}

/// Create the weekly digest's session and row together; the turn itself
/// starts in [`advance_weekly`], through the same claim-guarded path spawns
/// use.
async fn start_weekly(project: &LocalProject, due: &Due) -> Result<()> {
    let kind = DigestKind::Weekly.as_str();
    {
        let store = Store::open()?;
        if store.get_digest(&project.id, kind, &due.period)?.is_some() {
            return Ok(());
        }
        let context = gather(&store, project, due, &store.digest_session_ids()?)?;
        if context.is_empty() {
            store.claim_digest(&project.id, kind, &due.period, "skipped", None)?;
            return Ok(());
        }
    }
    let Some(agent) = crate::local::starter::resolve_agent().await else {
        return Ok(());
    };
    let store = Store::open()?;
    // Re-checked: `resolve_agent` awaited, and another tick may have won.
    if store.get_digest(&project.id, kind, &due.period)?.is_some() {
        return Ok(());
    }
    // Nobody watches this turn, so it must not stop on a permission prompt.
    let permission_mode =
        crate::local::harness::permission_id_for_mode(&agent.harness, PermissionMode::Auto)
            .or_else(|| {
                crate::local::harness::permission_id_for_mode(
                    &agent.harness,
                    PermissionMode::Bypass,
                )
            });
    let title = format!("{} \u{b7} {}", due.title, project.name);
    let session = StoredChatSession {
        id: format!("chat_{}", uuid::Uuid::new_v4()),
        project_id: project.id.clone(),
        harness: agent.harness.clone(),
        native_session_id: None,
        title: Some(title),
        title_source: Some("user".to_string()),
        model: agent.model.clone(),
        service_tier: None,
        permission_mode,
        plan_mode: false,
        plan_reset_pending: false,
        reasoning_level: None,
        archived: false,
        context_usage_json: None,
        bootstrap_context: None,
        goal: None,
        active_leaf_id: None,
        parent_session_id: None,
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    let tx = store.begin()?;
    store.create_chat_session(&session)?;
    store.claim_digest(&project.id, kind, &due.period, "pending", Some(&session.id))?;
    tx.commit()?;
    Ok(())
}

/// Start pending weekly turns; post the ones that have finished.
async fn advance_weekly(chat: &Arc<ChatHost>) -> Result<()> {
    let kind = DigestKind::Weekly;
    let pending = Store::open()?.list_digests_in_state(kind.as_str(), "pending")?;
    for row in pending {
        let Some(session_id) = row.session_id.clone() else {
            continue;
        };
        let (project, prompt, record_brief) = {
            let store = Store::open()?;
            let Some(project) = store.get_local_project(&row.project_id)? else {
                store.set_digest_state(&row, "pending", "failed")?;
                continue;
            };
            let Some(due) = weekly_due_for(&row.period) else {
                store.set_digest_state(&row, "pending", "failed")?;
                continue;
            };
            let context = gather(&store, &project, &due, &store.digest_session_ids()?)?;
            let record_brief = store.list_chat_messages(&session_id)?.is_empty();
            let prompt = weekly_prompt(&project, &due, &context);
            (project, prompt, record_brief)
        };
        let Some(guard) = TurnGuard::claim_hidden(chat, &session_id).await else {
            continue;
        };
        chat.emit_session(Store::open()?.get_chat_session(&session_id)?)
            .await;
        let started = chat
            .send_spawn_task(&session_id, prompt, record_brief, guard)
            .await;
        let store = Store::open()?;
        match started {
            Ok(TurnSubmission::Started(_)) => {
                store.set_digest_state(&row, "pending", "running")?;
            }
            outcome => {
                if let Err(err) = &outcome {
                    eprintln!(
                        "orx up: could not start weekly digest for {}: {err}",
                        project.name
                    );
                }
                if store.record_digest_attempt(&row)? >= MAX_ATTEMPTS {
                    store.set_digest_state(&row, "pending", "failed")?;
                }
            }
        }
    }

    let running = Store::open()?.list_digests_in_state(kind.as_str(), "running")?;
    for row in running {
        let Some(session_id) = row.session_id.clone() else {
            continue;
        };
        let busy =
            chat.is_busy(&session_id).await || Store::open()?.chat_turn_leased(&session_id)?;
        if busy {
            let stuck = row
                .started_at
                .is_some_and(|started| now_ms() - started > WEEKLY_TIMEOUT_MS);
            if stuck {
                let _ = chat.interrupt(&session_id).await;
                Store::open()?.set_digest_state(&row, "running", "failed")?;
                eprintln!("orx up: weekly digest {session_id} timed out; stopped it");
            }
            continue;
        }
        let store = Store::open()?;
        let (Some(project), Some(session)) = (
            store.get_local_project(&row.project_id)?,
            store.get_chat_session(&session_id)?,
        ) else {
            store.set_digest_state(&row, "running", "failed")?;
            continue;
        };
        match closing_outcome(&store, &session, final_answer_text)? {
            SpawnOutcome::Reply(text) => {
                let title = weekly_due_for(&row.period)
                    .map(|due| due.title)
                    .unwrap_or_else(|| "Weekly digest".to_string());
                crate::notify_events::enqueue_digest(
                    &store,
                    &project,
                    kind.notification_kind(),
                    &title,
                    &text,
                )?;
                store.set_digest_state(&row, "running", "done")?;
            }
            _ => {
                store.set_digest_state(&row, "running", "failed")?;
                eprintln!("orx up: weekly digest {session_id} ended without a reply");
            }
        }
    }
    Ok(())
}

/// Rebuild a weekly `Due` from its stored period (that week's Monday), so a
/// turn started on one tick is prompted and titled the same on a later one.
fn weekly_due_for(period: &str) -> Option<Due> {
    let monday: jiff::civil::Date = period.parse().ok()?;
    let at = monday
        .at(DIGEST_HOUR, 0, 0, 0)
        .to_zoned(jiff::tz::TimeZone::system())
        .ok()?;
    due_digest(&at, true).filter(|due| due.kind == DigestKind::Weekly)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Zoned {
        s.parse().unwrap()
    }

    #[test]
    fn nothing_is_due_before_nine_or_on_weekends() {
        assert_eq!(
            due_digest(&at("2026-09-24T08:59[America/New_York]"), true),
            None
        );
        assert_eq!(
            due_digest(&at("2026-09-26T10:00[America/New_York]"), true),
            None
        );
        assert_eq!(
            due_digest(&at("2026-09-27T10:00[America/New_York]"), true),
            None
        );
    }

    #[test]
    fn a_weekday_gets_a_daily_covering_this_week_so_far() {
        let now = at("2026-09-24T09:30[America/New_York]");
        let due = due_digest(&now, true).unwrap();
        assert_eq!(due.kind, DigestKind::Daily);
        assert_eq!(due.period, "2026-09-24");
        assert_eq!(due.title, "Daily digest \u{b7} Thu Sep 24");
        let monday = at("2026-09-21T00:00[America/New_York]");
        assert_eq!(due.window_start_ms, monday.timestamp().as_millisecond());
        assert_eq!(due.window_end_ms, now.timestamp().as_millisecond());
    }

    #[test]
    fn monday_gets_only_the_weekly_covering_last_week() {
        let monday = at("2026-09-28T11:00[America/New_York]");
        let due = due_digest(&monday, true).unwrap();
        assert_eq!(due.kind, DigestKind::Weekly);
        assert_eq!(due.period, "2026-09-28");
        assert_eq!(due.title, "Weekly digest \u{b7} week of Sep 21");
        assert_eq!(
            due.window_start_ms,
            at("2026-09-21T00:00[America/New_York]")
                .timestamp()
                .as_millisecond()
        );
        assert_eq!(
            due.window_end_ms,
            at("2026-09-28T00:00[America/New_York]")
                .timestamp()
                .as_millisecond()
        );
    }

    #[test]
    fn monday_falls_back_to_a_daily_when_the_weekly_is_off() {
        let due = due_digest(&at("2026-09-28T11:00[America/New_York]"), false).unwrap();
        assert_eq!(due.kind, DigestKind::Daily);
        assert_eq!(due.period, "2026-09-28");
    }

    #[test]
    fn a_stored_weekly_period_rebuilds_the_same_due() {
        let due = weekly_due_for("2026-09-28").unwrap();
        assert_eq!(due.period, "2026-09-28");
        assert_eq!(due.title, "Weekly digest \u{b7} week of Sep 21");
        assert!(weekly_due_for("2026-09-29").is_none());
    }

    #[test]
    fn a_digest_row_is_claimed_once() {
        let dir = std::env::temp_dir().join(format!("orx-digest-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        assert!(store
            .claim_digest("p1", "daily", "2026-09-24", "running", None)
            .unwrap());
        assert!(!store
            .claim_digest("p1", "daily", "2026-09-24", "running", None)
            .unwrap());
        let row = store
            .get_digest("p1", "daily", "2026-09-24")
            .unwrap()
            .unwrap();
        assert!(store.set_digest_state(&row, "running", "done").unwrap());
        assert!(!store.set_digest_state(&row, "running", "done").unwrap());
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}
