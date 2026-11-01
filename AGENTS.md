# Current round coordination (temporary)

## Direction (2026-11-01)

The `cli` branch is **CLI-only**. Do not add or extend desktop integration here.
Plan B was executed: `crates/app` (GPUI), `src-tauri`, `dist/`, `assets/`,
`release-macos.yml` and the desktop scripts were deleted from this branch. The
workspace is `crates/core` + `crates/cli` only. The desktop implementation is
still available in the `main` branch history if ever needed. The desktop
login-session, app-owned upload/queue migration, and desktop manual acceptance
items are cancelled for this branch.

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
  `state`, `diagnostics`, `cancellable_*`) are in scope; do not delete them as
  "desktop code" — several still carry desktop-era compatibility wrappers.

## Completed slices (do not redo)

Login delivery/owned portal, continuous read progress, post-write cancellation,
pre-write auth cancellation, read-only query cancellation, queue cancellation
with unknown semantics, and explicit dashboard refresh input/errors are done.
The desktop refresh *app wiring* is out of scope now; the core
`DashboardRefreshInput` / strict-error API stays.

## Current state

- Base/HEAD before this slice: `e9ff2ee` (= `origin/cli`).
- Desktop trees deleted; `Cargo.lock` pruned from 8958 to 2252 lines.
- `desktop-compatibility` CI job removed; CLI jobs unchanged.
- README and `docs/cli.md` describe CLI only.
- Unified check before the deletion: 267 Rust + 7 Python, exit 0
  (`/tmp/sandflos-cli-only-flakefix-check.log`).
- Outstanding: real-platform acceptance (credentials, QR, BOOX/OSS/WebDAV,
  device receipt, tag→draft release).

## Next work

Only CLI-scope work. Candidate items are listed in
`docs/plans/cli-next-steps-and-debt.md` — ignore its desktop entries.
