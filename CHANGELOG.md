# Changelog

## [0.5.4]

Standing instructions for agents. A new Instructions tab on the Projects page, next to Slack, holds rules every agent follows in every project, and each project's Settings has an Instructions section for rules that apply to that project alone (project rules win on conflict). That section mirrors the `AGENTS.md` at the project's root: it is created if missing, edits are written to the file, and changes made to the file show up in Settings. The file is left uncommitted for you to commit. They are added to each agent's startup prompt, so changes reach new chats and any agent that restarts.

Until you save your own, the global instructions show a built-in default: keep prose `EXPERIMENT_PLAN.md` and `EXPERIMENT_LOG.md` files in the project's artifacts directory, report each substantial experiment in the log with what was run, example inputs and outputs, metrics and findings, and keep multi-job experiments out of the chat by answering a single finished job in a sentence or two with the detail in the log. "Restore default" brings it back.

The message an agent receives when a run finishes now asks for one to three short sentences and no restating of earlier reports, instead of "a few sentences".

## [0.5.3]

Per-project Slack settings now also apply to messages queued by a run supervisor that started before an update: the channel and the muted events are applied when the message is sent, so a project's override no longer depends on which version queued it. Restarting a run's supervisor (Resync on the run) still picks up everything else new.

The "no new job output" stall notice no longer fires on a job that is writing. The supervisor's copy of a log only moves when a complete line arrives, so a job drawing a progress bar looked silent; it now asks the cluster how long its log has gone unwritten before announcing anything. The silence threshold is raised from 30 minutes to 2 hours.

Run-launch messages with a long experiment title are no longer refused by Slack. Its header line is limited to 150 characters, and a longer one made Slack reject the whole message; the header is now shortened and the full line moves to the top of the message body.

## [0.5.2]

Slack settings per project, in each project's Settings: send a project's messages to its own channel (needs the bot token), choose who may reply in its threads, and turn any notification or digest off for that project alone. A message goes out only when it is on both globally and for the project; anything not set for a project follows the global settings, which now have their own Slack tab on the Projects page.

## [0.5.0]

Reply to orx from Slack: with the Slack app's bot and app-level tokens saved, orx posts as the app, and a reply in a run message's thread from an allow-listed member goes to the chat that ran it. The agent's answer is posted back into the thread. One machine holds the Socket Mode connection ("Receive replies on this machine"), and replies sent while it was offline are picked up when it reconnects. The app needs the `message.channels` event (`message.groups` for private channels); see `docs/slack-app-manifest.yaml`.

## [0.4.1]

Slack digests now report the key learning since the previous digest and a running "best so far" commentary on the project's headline metric (F1 Macro where reported). Each digest builds on the last one Slack actually delivered, and marks what is new since then.

## [0.4.0]

Slack digests, one per project: a weekday-morning digest of runs in flight and open next steps from this week's chats, and a Monday digest of last week's progress, open next steps, and a search for relevant new work.

## [0.3.3]

Quieter Slack notifications: stalled-run pings at most once per run every 2 hours, a clearer "Launching experiment run" header on launch messages, and run-synthesis messages cut down to a short, plain summary of what was run and the result.

## [0.3.0]

Resetting version number to reflect minor addition: SGE compute backend, improved job monitoring and wake-ups.
