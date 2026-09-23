# Repository Guide

## What this repository is

This is `gardoslab/OpenResearch`, a fork of `alphaXiv/OpenResearch` modified for our own research at BU. It contains `openresearch-cli`, the Rust implementation of the `orx` command-line tool: the local CLI, dashboard and API, SQLite store, coding-agent integrations, experiment orchestration, and execution backends. Fork-specific work includes the SGE backend for the SCC, run status fixes, usage-limit auto-continue, `@file` mentions, and Slack notifications.

Git remotes: `gardoslab` is this fork (the one we push to and release from), `upstream` is alphaXiv, and `origin` is a personal copy. Bring upstream changes in by merging `upstream/main` into `dev`, then check for semantic breaks the merge cannot see (`cargo clippy`, `tsc`, and the i18n check when upstream adds locales).

`openresearch.sh` is the upstream companion service. It owns the website and documentation, accounts and organizations, sandbox provisioning, managed-compute catalogs, and the telemetry endpoint. Research projects, experiments, runs, logs, and artifacts remain local to `orx`. When changing authentication, organization, sandbox, or managed-compute APIs, inspect the corresponding `openresearch.sh` implementation and keep both sides compatible. Do not edit the companion repository unless it is explicitly in scope.

## Branches

- `dev` is what gets released. Every merge to `dev` goes through a pull request, and a version bump there publishes a release.
- Add features and fixes on topic branches off `dev` (`git switch dev && git switch -c my-feature`), and open the pull request against `dev`.
- `main` only tracks the parent, `upstream` (alphaXiv/OpenResearch) — nothing of ours merges into it, and it is never released. Keep it current with `git merge upstream/main` (fast-forward when possible) pushed straight to `main`; it carries no fork-specific changes, so it needs no PR.
- Bring upstream changes into our own work by merging `upstream/main` into `dev` (see above), independent of keeping `main` in sync.
- That merge also updates `UPSTREAM_VERSION` at the repository root to the upstream release `dev` now carries, in the same pull request. `build.rs` bakes it in so `orx --version` reports `orx <ours> (upstream <theirs>)`. It is an annotation only — never compared against anything — so our own `Cargo.toml` version stays a plain semver line and a stale value cannot affect update checks.

## Development guidelines

- Rust code lives in `src/`; the dashboard lives in `ui/src/`. Keep local-only behavior local and use the production API client only for capabilities owned by `openresearch.sh`.
- Run local app instances through `scripts/dev-slot.mjs` (`just up`) so development data, ports, and processes stay isolated.
- `ui/dist` is committed and embedded in release builds. After UI changes, run `pnpm build` in `ui/` and include the regenerated assets.
- Every user-facing string goes in `ui/messages/*.json` for all locales; `node ui/scripts/check-i18n.mjs` enforces it.
- Prefer canonical Tailwind utilities (`flex flex-col h-full min-h-0`) and project theme aliases (`bg-background`, `text-subtext`, `border-border`). Use arbitrary values only when no project utility exists, and preserve semantic marker classes when selectors or runtime behavior depend on them.
- Code used only on unix must be `#[cfg(unix)]`, including its constants. Windows CI builds with `-D warnings`, so an unused item fails the release.
- Before pushing, follow the checks in `.github/workflows/ci.yml`.

## CI and release gates

- GitHub protection for `dev` should require the `fmt, clippy, test` and `version sanity` checks from GitHub Actions, including for administrators. Do not require a merge queue or require branches to be up to date. `main` needs no such protection — it only ever receives fast-forwards from `upstream/main`. These settings are managed in GitHub, not by this file.
- PR CI must test GitHub's simulated merge (`refs/pull/<number>/merge`), which `actions/checkout` selects by default for `pull_request` events, rather than checking out the PR head alone. Each run tests its merge candidate; subsequent changes to `dev` do not automatically rerun open PRs.
- CI also runs on `dev`. Releases call the same CI workflow on the commit being packaged; publishing requires that run to succeed. Keep `./ci` in cargo-dist's `global-artifacts-jobs` when regenerating the release workflow.
- Releases come from `release-on-bump.yml`: when a push to `dev` changes `version` in `Cargo.toml`, it dispatches `release.yml`, which builds Linux, macOS and Windows binaries and publishes the GitHub Release. Bumping the version is the release act, and the version is always a human decision in a reviewed PR.
- A version bump PR must also update the lock file: run `cargo update -p openresearch-cli --offline` and commit `Cargo.lock`. CI uses `--locked`, so a stale lock fails the release.
- If a release run fails, do not re-run it: a re-run replays the old commit. Merge the fix, then run `gh workflow run release.yml --ref dev -f tag=vX.Y.Z` to build `dev` again under the same tag.
- `build.rs` only allows production builds from repositories listed in `OFFICIAL_REPOS` and bakes the building repository into the binary, so `orx update` follows the fork that built it. Adding another release repository means adding it there.
- `repository` in `Cargo.toml` is what cargo-dist writes into the installer script's download URL. Keep it pointing at `gardoslab/OpenResearch`.
- Only installer-managed binaries can self-update. A `cargo install` or `cargo run` build cannot, so servers that should update from the dashboard need the installer.
- The macOS DMG job stays off unless the `MACOS_SIGNING_ENABLED` repository variable is set, which we do not need.
