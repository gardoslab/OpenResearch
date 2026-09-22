# Multi-user OpenResearch on a shared machine

*Status: proposal. Written 2026-09-22 for the BU fork. No code has been written
for any tier below.*

Several of us work on the same Ubuntu machine, each with our own account. Today
each person's `orx` is an island: you cannot see a colleague's experiments,
their run results, or even that they have `orx` running. This document explains
why that is, what upstream has decided about it, and four possible levels of
response.

## What upstream has decided

OpenResearch is single-user by design, and the design was made deliberately and
recently.

In August 2026, upstream landed `505b272 refactor: keep research state local
(#202)`, which removed the server-side projects, experiments, runs and artifacts
APIs — 779 lines out of `src/client.rs`. A test now fails the build if any of
them return:

```rust
for forbidden in ["\"/projects", "\"/experiments", "\"/runs", "\"/skills"] {
    assert!(!production.contains(forbidden),
        "research-state endpoint remains in client.rs: {forbidden}");
}
```

What remains on `openresearch.sh` is `/orgs`, `/compute/catalog`, `/sandboxes`
and `/ssh-keys`. Organizations therefore *do* exist upstream, but only as a
scope for picking managed compute; `src/commands/orgs.rs` is 27 lines that list
org IDs. They carry no shared research state.

The same commitment runs through the rest of the code:

- `src/commands/remote_host.rs` describes its control channel as "private
  same-user" and enforces that with `geteuid()` comparisons, `SO_PEERCRED`
  checks, `0o600`/`0o700` modes and `O_NOFOLLOW`.
- The store has no owner column anywhere. `local_projects`,
  `local_experiments` and `chat_sessions` are single-tenant by construction.
- The data directory is per-user: `$ORX_DATA_DIR`, else
  `$XDG_DATA_HOME/openresearch`, else `~/.local/share/openresearch`.

On the issue tracker the closest prior art is
[#316](https://github.com/alphaXiv/OpenResearch/issues/316), proposing a
scheduler for "one big shared box, several researchers, no cluster admin". That
is about GPU contention rather than visibility, and a maintainer's reply was in
effect *wait for more demand*. There are no discussions enabled on the
repository and no other multi-user issues.

**Conclusion: there is no upstream foundation to build on, and anything we add
here diverges from a direction upstream committed to explicitly.** That matters
for this fork in particular, because we merge `upstream/main` regularly.

## Why you cannot see a colleague's work

Three independent reasons, each sufficient on its own:

1. Their `orx.db` lives under their home directory, in their own data dir.
2. `orx up --remote user@host` starts `orx` on the remote machine *as you*, so
   it opens your data dir there, not theirs.
3. Nothing in the schema or the local API can express "another person's
   experiment", so there is no read path to expose even if the file were
   readable.

## Four tiers of response

### Tier 0 — use git (no code changes)

The experiment tree is already git-native: every experiment is a branch, and
every run archives the commit it recorded. If we add a shared bare remote and
push experiment branches, we get the *code* half of visibility immediately, with
full history and no new software.

What it does not give us: run status, logs, results, artifacts and chat
transcripts, all of which live only in each person's SQLite store.

Cost: an afternoon. Covers roughly half the need.

### Tier 1 — read-only snapshot export

Add `orx export --out <path>`: WAL-checkpoint the database, then copy `orx.db`
along with `run-logs/` and `files/` to a group-readable location.
`src/local/datadir.rs` already performs exactly this checkpoint-and-copy when
relocating a data directory, so the mechanism exists and is tested.

A colleague can then browse it:

```sh
ORX_DATA_DIR=/shared/exports/alice orx up --port 4792
```

Two caveats:

- It is point-in-time, not live. SQLite in WAL mode cannot be safely read across
  users on a database someone else is writing.
- It needs a companion `--read-only` flag that disables run launching and chat,
  so the UI does not offer actions that would fail — or, worse, write into
  someone else's export.

Cost: days, not weeks. Self-contained, low merge risk.

### Tier 2 — a lab view (recommended)

A small separate service ingests everyone's Tier-1 exports on a timer and serves
one combined, read-only dashboard: who is running what, which experiments exist,
which results have landed.

This is the shape that actually answers *"what has everyone been working on and
what did they find"*. Critically, it does not touch `orx`'s single-user model at
all, so it survives upstream merges untouched, and it could live outside this
repository entirely.

Cost: weeks. Highest value per unit of maintenance risk.

### Tier 3 — true multi-user orx (not recommended)

Owner columns on every table, real identity, authentication and authorization on
every `/api/*` route, per-user agent process isolation, and a permission model
governing whose credentials an agent spawns under.

This is a large change to the files that churn most upstream — `src/commands/up.rs`
alone is roughly 8,700 lines and changes constantly. It contradicts the
direction upstream committed to in #202, and it would make every
`upstream/main` merge painful indefinitely.

If we ever do want it, the existing `--remote-host` bearer mode is the
foundation to build on rather than starting fresh: it already has a session
token, and both `DASHBOARD_PROTOCOL` and `CONTROL_PROTOCOL` version negotiation.

## Recommendation

Do Tier 0 now. Treat Tier 2 as the real project, with Tier 1 as its first
deliverable since the lab view needs the exports anyway. Avoid Tier 3 unless
the lab view proves insufficient in practice.

Separately, consider opening an issue upstream describing the lab scenario.
Issue #316 drew "let's see if more people ask"; a second concrete demand signal
from a university lab may move that. A feature built upstream costs us nothing
to maintain.

## Open questions

- Do people want to *see* each other's chat transcripts, or only experiments and
  results? The latter is a much smaller surface and avoids most privacy
  questions.
- Should exports be opt-in per project, or whole-store?
- Where would a lab view live — this repository, a separate one, or a service on
  the shared machine?
