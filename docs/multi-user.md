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

### Tier 0 — turn on what already exists (recommended first step)

#### GitHub syncing

`orx` can already do this, and it is off by default. Settings has a "GitHub
publishing" toggle: with it on, each new project gets a private GitHub
repository and experiment branches are pushed automatically. Creating an
experiment and submitting a run each spawn a detached `orx publish-branch`
worker that runs `git push -u <remote> <branch>`; the branches selected are the
project's baseline plus everything under `orx/`.

"Collaborator visibility" in that setting's description means ordinary GitHub
sharing — whoever you add as a collaborator on the private repository can see
the branches. `orx` never reads anyone else's synced repository, so this is a
publishing mechanism, not a shared view.

What crosses: experiment branches, every commit an agent made, full diffs and
history. That is genuinely most of *what* someone tried.

What does not cross, because it lives only in each person's `orx.db` and
`run-logs/`: run status and exit codes, run logs, results, artifacts, chat
transcripts, experiment titles and descriptions, and the parent/child lineage of
the experiment tree — branch names encode an experiment's identity, not its
`parent_experiment_id`.

So Tier 0 answers "what have they been working on" and leaves "what did they
find" unanswered. That gap is the whole case for Tiers 1 and 2.

Two caveats worth knowing:

- Project repositories are created under the *personal* account of whoever
  creates them: `create_project_repo` in `src/local/github.rs` calls
  `viewer_login()` and creates `<your-login>/<repo> --private`. Every project
  therefore becomes a repository under an individual's account, and collaborators
  are added by hand, one repository at a time. For a lab, repositories under a
  shared organization would be markedly better — see the proposal below.
- Pushes are fire-and-forget: the worker is detached with stdout and stderr
  discarded, and a failure surfaces only as a single line on stderr. There is a
  `publication_sync_status` helper that checks with `ls-remote`, but do not
  assume a branch arrived without looking.

#### Proposal: let new repositories target a shared organization

A small, self-contained change, following the existing `github_for_new_projects`
setting end to end:

1. `github_org: Option<String>` on the `Settings` struct in `src/telemetry.rs`,
   with a getter that treats an empty string as unset, plus a setter, both
   routed through `mutate_settings` like every other field.
2. Re-export both through `src/config.rs`.
3. A `project_repo_owner()` helper in `src/local/github.rs` returning the
   configured organization, else `viewer_login()`. Both `create_project_repo`
   and `available_project_repo_name` call it, so choosing a name and creating
   the repository cannot disagree.
4. Surface `githubOrg` on the existing `/api/settings/projects` GET and accept
   it on the POST, in `src/commands/up.rs`.
5. A text field beside the existing toggle in `SettingsPage.tsx`, with the
   matching `ui/messages/*.json` keys for all six locales, and a rebuilt
   `ui/dist`.

Blank preserves today's behaviour exactly, so the change is additive. Worth
surfacing a clear error when the organization is misspelled or the account
cannot create repositories in it, since `gh repo create` fails at that point
rather than where the name was chosen. Plausibly something upstream would take.

#### Experiment tracking

Our training code already logs to Weights & Biases, and `orx` steers toward it.
Both references are upstream's, not additions of this fork:

- The agent skill in `src/local/skills.rs` tells agents "Optional tracking: if
  the user wants metrics logged, prefer Weights & Biases — check `wandb login` /
  `WANDB_API_KEY` and log each run to a project named after the paper. Don't
  require it." That line arrived in `e8972bb` on 2026-07-10 and is still in
  `upstream/main`.
- `WANDB_API_KEY` is one of three `RECOMMENDED_ENV_KEYS` in the Settings UI,
  alongside `TINKER_API_KEY` and `HF_TOKEN`, in `upstream/main` as well as here.

The sequence is what makes this more than a coincidence: upstream shipped that
guidance in July 2026, then deleted server-side run state a month later in
`505b272` (2026-08-17). Read together, the split is deliberate — `orx` owns
code, experiments and orchestration; a tracker owns metrics and results.

Two honest qualifications. The skill line is hedged ("Optional... Don't require
it"), so it is a default preference rather than an architectural commitment, and
upstream has nowhere stated outright that results belong in W&B. This is an
inference from what they shipped, not a documented position.

Pointed at a shared W&B team rather than personal entities, this covers most of
what the tiers below were reaching for: metrics, curves, run configuration,
system stats, artifacts and stdout, all visible to everyone in the team, live,
with no new software and no fork changes.

Cost: none, both exist. Together they cover most of the need.

### What Tier 0 still leaves out

With GitHub syncing and a shared W&B team both on, the residual gap is narrow.
It is worth naming precisely, because it is what Tiers 1 to 3 would buy:

- **The join between the two halves.** Nothing connects a W&B run back to the
  `orx` experiment, branch and commit that produced it, unless the training code
  logs that itself. Without it you can see someone's curves and someone's
  branches but cannot reliably tell which produced which.
- **Runs that fail before training starts.** A submit error, an SGE queue
  rejection or a crash before `wandb.init()` never reaches W&B, and its status
  and logs stay in the launching user's `orx.db` and `run-logs/`.
- **Chat transcripts and the agent's reasoning.** Why an experiment was tried,
  what the agent considered and rejected. This is `orx`-specific, has no
  equivalent in either tool, and is arguably the most novel thing the workspace
  records.
- **Experiment tree lineage.** Which experiment was forked from which. Branch
  names encode an experiment's identity but not its `parent_experiment_id`.
- **Titles and descriptions**, which live only in the local store.

The first item is much the most valuable and much the cheapest: have the
training code pass the `orx` experiment id, branch and commit into
`wandb.init(config=...)` or as tags. That is a change to our own training code,
not to `orx`, and it turns two partial views into one navigable one.

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

Cost: days, not weeks. Self-contained, low merge risk — but only worth starting
if Tier 2 is going ahead, since the lab view is what consumes the exports.

### Tier 2 — a lab view (only if Tier 0 proves insufficient)

A small separate service ingests everyone's Tier-1 exports on a timer and serves
one combined, read-only dashboard: who is running what, which experiments exist,
which results have landed.

Before W&B entered the picture this looked like the answer to *"what did they
find"*. With a shared W&B team carrying results, what it adds over Tier 0 is
chat transcripts, experiment-tree lineage and the runs that died before training
started. Its merit is that it does not touch `orx`'s single-user model at all,
so it survives upstream merges untouched and could live outside this repository
entirely.

Cost: weeks, plus a service to keep running. Hard to justify until the Tier 0
gap is felt.

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

Do Tier 0, and stop there for now.

Turn on GitHub syncing, point W&B at a shared team, and make the training code
stamp each W&B run with its `orx` experiment id, branch and commit. That is a
few hours of work, no fork changes, and it covers code, results and the link
between them — which was the whole question.

Then use it for a few weeks and see what is actually missing. The honest case
for Tiers 1 and 2 is much weaker once W&B is carrying the results: they would
mainly add chat transcripts, tree lineage and the status of runs that died
before training began. That may well not be worth a service to maintain. Build
them only if the gap bites in practice, and Tier 3 only if the lab view proves
insufficient after that.

The organization-target change under Tier 0 is worth doing on its own merits:
it is small, additive, and makes GitHub syncing behave sensibly for a group
rather than an individual.

Separately, consider opening an issue upstream describing the lab scenario.
Issue #316 drew "let's see if more people ask"; a second concrete demand signal
from a university lab may move that. A feature built upstream costs us nothing
to maintain.

## Open questions

- Are we all logging to a shared W&B team already, or to personal entities? The
  whole Tier 0 argument depends on the former.
- Does the training code already record the `orx` experiment or commit in the
  W&B run config? If not, that is the single highest-value change here.
- Do people want to *see* each other's chat transcripts, or only experiments and
  results? The latter is a much smaller surface and avoids most privacy
  questions.
- Should the organization target be a global default, or selectable per project
  when a project is created?
