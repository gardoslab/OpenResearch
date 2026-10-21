---
name: orx-ui
description: "Show an interactive widget in the chat and get the user's choice back: compare outputs from several models or configurations side by side, review images or videos, pick a winner, adjust a plot's parameters before publishing. Use when a decision is easier to make by looking and clicking than by reading text; not for static results, which belong in a report."
---

A fenced block whose language is `orx-ui` renders as a live HTML page inside the
chat message. Use it when the user must look at something to decide: two model
outputs, a set of generated images, a video before and after a change.

## Writing a widget

````
```orx-ui
<style>body{font-family:sans-serif;margin:12px}</style>
<video src="artifacts/run1/out.mp4" controls width="240"></video>
<video src="artifacts/run2/out.mp4" controls width="240"></video>
<button onclick="orx.submit({winner:'run1'})">run1 is better</button>
<button onclick="orx.submit({winner:'run2'})">run2 is better</button>
```
````

- It is ordinary HTML, CSS and JavaScript in one block. The page has a white
  background; set your own colors.
- `orx.submit("text")` or `orx.submit({key: value})` hands the answer to the
  user. It fills the chat composer; the user reads it and presses send, so it
  arrives as their next message, starting `Widget response:`.
- Reference project files by the same relative paths you use in markdown images
  (`artifacts/...`). `img`, `video`, `audio`, `source` and `link`/`script` files
  are embedded for you, up to about 4 MB in total, so keep media small (a short
  clip, a thumbnail grid). Link to larger files with a normal file mention
  instead.
- The widget runs sandboxed: no network to the dashboard, no cookies or storage,
  no access to the project beyond what you embed. Do not put secrets in it.
- It is re-rendered when the message changes and does not keep state across
  reloads, so put anything the user needs to remember in the page itself.

## Before you use it

1. Ask for the decision in prose first, then show the widget. The widget is an
   aid, not the whole message.
2. Always include a control that calls `orx.submit`; a widget with no way back is
   just a report.
3. Keep it to one decision. After the user answers, act on it and say what you did.
