# Auth cancellation and migration atomicity (2026-11-04, final)

Scope: `crates/core/src/auth_service.rs`, `crates/cli/src/auth.rs`,
`crates/cli/src/config.rs`, and the SIGINT handler installation in
`crates/cli/src/main.rs`. Base HEAD for the slice was `46fad34`. This report consolidates and replaces the earlier `tdd-auth-migration.md`; it retains only evidence that exists on disk or was reproduced in this final pass.

## Public seams

- **S1 CLI:** compiled `booxctl` subprocess, stdin, environment-selected file
  store, and a bounded synthetic loopback auth server. Assertions cover exit
  codes, JSON output, whether `auth.json` exists, and a fresh `auth status`
  process.
- **S2 core:** `AuthService` with a blocking fake verifier and a real
  `FileCredentialStore` in a temporary directory. No real accounts, keychain
  entries, books, or network endpoints were used.

## TDD evidence limits

Inherited implementation had no independently preserved behavioral Red log. `/tmp/booxctl-auth-cancel-red.log` contains only a build-lock notice; it is not Red evidence. The earlier release review reproduced persistence after cancellation and concurrent migration overwrite, but those probes are review evidence, not proof that these tests preceded implementation. This round preserves the working changes and validates them with regressions; no rollback was used to manufacture Red.

## Green

After the fix:

```bash
cargo test --locked -p boox-core auth_service::tests
cargo test --locked -p boox-core --test auth_qr_failures
cargo test --locked -p booxctl --test auth_cancel_release
cargo clippy --locked -p boox-core -p booxctl --all-targets -- -D warnings
```

The full isolated-worktree log is `/tmp/auth-migrate-tests.log` (created
2026-11-04 07:27). It records 83 core unit tests, 3 QR failure tests, and 3
end-to-end auth tests passing. It also records five **queue** test failures:
that worktree carried the new queue production behavior but old queue test
expectations (observed exit 6 versus expected 130). Those are unrelated to
auth and are not evidence that the auth slice is green on the current HEAD.

This final pass reran:

```bash
cargo test --locked -p boox-core auth_service::tests
cargo test --locked -p booxctl --test auth_cancel_release
```

The core auth filter passed on its own; the end-to-end auth suite passed
3/3 in 1.07s, including the concurrent migration case. Strict clippy over both
crates was attempted, but a concurrent edit to `crates/core/src/device.rs`
left that file temporarily uncompilable (`stdout??` on an `Option`), so clippy
must be rerun by the parent after that edit settles. No auth code was involved
in that failure.

The final fixture rewrite makes every acceptance HTTP operation bounded:
nonblocking accept with an absolute start deadline, per-connection read and
write timeouts, a five-second request-header and release deadline, and real
joins on drop. The migration regression now requires **two** concurrent
verification requests to be accepted and held before either response is
released, so it cannot pass by running the migrations sequentially.

The end-to-end cases assert:

- SIGINT after the verification response returns `cancelled` with exit **130**
  and no credential is written.
- SIGINT while stdin remains open exits within the bounded deadline.
- Two migrations held at verification cannot both create the target. Exactly
  one exits **0**; the loser exits **2** (`credentials_already_exist`) or **5**
  (`credential_store_busy`) and the first credential remains intact.

## Limits

- The file backend performs the create-if-absent check and write under
  `auth.lock`, so concurrent file-store migrations have an atomic no-overwrite
  guarantee. Keyring mutations now share a config-directory lock across the absent check and write; real platform operation remains unverified.
- The bounded stdin cancellation path is Unix-specific. Windows Ctrl-C at the
  stdin boundary and real Credential Manager behavior still require manual
  validation. The non-Unix path reads to EOF and decodes UTF-8 after all bytes.
- A descriptor that is already `O_NONBLOCK` still enters the polling loop;
  the RAII guard restores the inherited flags whether or not it changed them.
- `KeyringCredentialStore` now fails with `config_dir_unavailable` when no
  configuration directory exists for its lock, instead of silently skipping
  the lock. On macOS and Windows the check and keyring write share that lock.
  Keyring deletion and replacement also take it. External keyring writers do not participate in this application lock.
- Cancellation during an already blocking HTTP request waits for that request
  or its configured timeout; the credential save is skipped once the request
  returns.
- The migration success path still preserves the source file byte for byte.
