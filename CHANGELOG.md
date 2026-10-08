# Changelog

## [0.5.2]

Slack settings per project (Settings → Slack → Per project): send a project's messages to its own channel (needs the bot token), choose who may reply in its threads, and turn any notification or digest off for that project alone. A message goes out only when it is on both globally and for the project; anything not set for a project follows the global settings.

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
