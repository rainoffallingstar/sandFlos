# Current round coordination (temporary)

## Direction (2026-11-01)

The `cli` branch is **CLI-only**. Do not add or extend desktop integration here.
`crates/app` (GPUI), `src-tauri`, `dist/`, desktop assets, `release-macos.yml`
and the desktop scripts remain in the repository for historical compatibility
and for `main`, but they are not build/test/CI/release targets of this branch.
The desktop login-session, app-owned upload/queue migration, and desktop manual
acceptance items are cancelled for this branch.

See `docs/plans/tdd-cli-only.md` for the cleanup record and evidence, and
`docs/plans/cli-github-status.md` for the CI state.

## Read first

- `docs/cli.md` — user-facing CLI contract (exit codes, JSON, cancellation).
- `docs/plans/cli-handoff.md` — history; its desktop sections are terminated.
- `docs/plans/cli-review-and-tdd.md` — S1–S4 public test seams.
- `docs/plans/tdd-cli-only.md` — current scope and verification.

## Working rules

- Scope is `crates/core` + `crates/cli` and the `sandflos` binary/packages.
- Use actual behavioral Red/Green. Compilation, setup, fixture, or missing-API
  failures are not Red. Never fabricate a Red by rolling back working code.
- Keep HTTP fixtures and child waits bounded: absolute read deadlines, bounded
  writes, real joins, synthetic loopback data only.
- No real accounts, credentials, books, tags, or releases.
- Do not `cargo fmt` shared production files while other writers are active.
- Implementation and review agents do not commit or push. Only the parent
  integrates, reviews, and commits/pushes `cli`.
- Document tests, exact commands, exit codes, log paths, and honest limits.
- `crates/core` modules used by CLI (`api`, `auth*`, `push`, `device`,
  `dashboard`, `zotero`, `calibre`, `cli_libraries`, `persistence`,
  `cancellable_*`) are in scope; do not delete them as "desktop code".

## Completed slices (do not redo)

Login delivery/owned portal, continuous read progress, post-write cancellation,
pre-write auth cancellation, read-only query cancellation, queue cancellation
with unknown semantics, and explicit dashboard refresh input/errors are done.
The desktop refresh *app wiring* is out of scope now; the core
`DashboardRefreshInput` / strict-error API stays.

## Current state

- Base/HEAD: `d3810d0fb5d9646bd18b49da8535a0c6c5cd52bc` (= `origin/cli`).
- `desktop-compatibility` CI job removed; CLI jobs unchanged.
- README and `docs/cli.md` describe CLI only.
- Commits `0113905` and `66b9bf2` are pushed to `origin/cli`.
- Unified check: 267 Rust + 7 Python, exit 0
  (`/tmp/sandflos-cli-only-flakefix-check.log`).
- Remote CI run `41257052317`: 6/6 success (three CLI jobs + three
  download/verify jobs; no desktop job).
- Outstanding: real-platform acceptance (credentials, QR, BOOX/OSS/WebDAV,
  device receipt, tag→draft release).

## Next work

Only CLI-scope work. Candidate items are listed in
`docs/plans/cli-next-steps-and-debt.md` — ignore its desktop entries.
