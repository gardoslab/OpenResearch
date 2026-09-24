<div align="center">

<h1><img src=".github/readme-assets/openresearch.svg" alt="" width="36" /> OpenResearch (BU fork)</h1>

**A local-first workspace for research agents and autoresearch.**

</div>

This is a fork of [alphaXiv/OpenResearch](https://github.com/alphaXiv/OpenResearch),
modified for our own research at BU. We keep it close to upstream and merge
upstream changes in regularly. Our additions so far include an SGE backend for
the SCC, job labels and status fixes for cluster runs, automatic continue after
usage limits, `@file` mentions in chat, Slack notifications, and in-app updates
from this fork's releases.

Releases are published at
[gardoslab/OpenResearch](https://github.com/gardoslab/OpenResearch/releases).
Upstream's docs at [openresearch.sh/docs](https://openresearch.sh/docs) still
apply to most of the app.

## Ways to run it

**1. Install a release (Linux, macOS, Windows).** This is the normal way, and
the only one where `orx update` and the dashboard's Update button work.

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/gardoslab/OpenResearch/releases/latest/download/openresearch-cli-installer.sh | INSTALLER_DOWNLOAD_URL=https://github.com/gardoslab/OpenResearch/releases/latest/download sh
orx up
```

The `INSTALLER_DOWNLOAD_URL` part is only needed for releases up to 0.2.9,
whose installer still points at upstream. Windows needs
[Git for Windows](docs/windows.md).

`orx up` opens the dashboard at `http://127.0.0.1:4791`. To update later, run
`orx update` or use the Update button in Settings.

**2. Run from source.** Needs Rust and, on Linux, `build-essential` and
`pkg-config`. The built dashboard is committed in `ui/dist`, so Node is not
required.

```sh
cargo run -- up            # run in place
cargo install --path . --locked   # or install orx into ~/.cargo/bin
```

A source build cannot update itself. Pull and rebuild instead.

**3. Development instance.** Runs a separate copy with its own ports, data and
processes, so it never touches your real projects. Needs Node, pnpm and
[`just`](https://github.com/casey/just).

```sh
just up      # start (or reuse) a dev slot on a copy of your database
just status
just down
```

**4. On a remote machine, browser on your laptop.** Install `orx` on the remote
machine (method 1 or 2), then from your laptop:

```sh
orx up --remote user@host
```

This starts `orx` on the remote machine and tunnels it to your browser. SSH
config aliases and custom ports work. The remote service listens on loopback
only and has no login, so other users on that machine can reach it.

If it reports that OpenResearch is not installed on the remote machine even
though `ssh user@host 'which orx'` finds it, the problem is permissions, not
PATH. Before launching, `orx` requires the remote binary to be owned by you or
root, and requires that neither the binary nor its parent directory is group-
or world-writable. A `cargo install` under Ubuntu's default umask of 002 leaves
`~/.cargo/bin` at mode 775, which fails that check:

```sh
ssh user@host 'chmod g-w ~/.cargo/bin'
```

The dashboard's Install button does not help here: it installs into that same
directory and then fails the same check, reporting that the installer finished
but no working `orx` binary was found.

## What it does

| | |
|---|---|
| **Parallel exploration** | Each research direction gets its own agent session and git worktree. |
| **Reproducible experiments** | A git-based experiment tree; every run gets an immutable archive of its commit. |
| **Evidence in context** | Logs, diffs, files and results stay tied to the work that produced them. |
| **Your choice of agent** | Claude Code, Codex, OpenCode, Cursor or Antigravity, chosen per session. |
| **Your choice of compute** | Local, SSH, Slurm, SGE, Kubernetes, Ray, Modal, Hugging Face Jobs and more. |
| **Local ownership** | Projects, chats, runs, logs and code stay on your machine, in a local SQLite store. |

Runs use a committed snapshot of your code, so nothing has to be published.

## CLI

```sh
orx install-skills          # add the OpenResearch skill to your coding agents
orx projects
orx runs <project-id>
orx logs <run-id>
orx exp run <experiment-id>
orx paper <arxiv-id-or-doi>
```

Run `orx --help` for everything else.

## Usage analytics

Release builds, including this fork's, send coarse, opt-out usage events tied to
a random installation ID to `api.openresearch.sh`. They contain no code,
prompts, file contents, paths, repository names, tokens or emails. Source and
development builds send nothing.

```sh
orx telemetry off
orx telemetry status
```

## Contributing

See [AGENTS.md](AGENTS.md) for the branch flow, checks and release process.

Coding agents may also file product feedback with `orx feedback` when you hit
a bug, wish for a feature, or get frustrated with OpenResearch. Each report is
a short description of the workflow problem, written to omit your research
details. Like analytics, reports are sent only from official release builds.
They are linked to your account when you are logged in and turned off by
`orx telemetry off`. The `--no-telemetry` flag covers only the command it is
passed to.
