# Zotero/WebDAV library finalization (2026-11-04)

Scope: `crates/core/src/cli_libraries.rs`, `crates/cli/src/zotero.rs`,
`crates/core/tests/cli_libraries.rs`, and `crates/cli/tests/library_cancel.rs`.
Base HEAD for the slice was `46fad34`. The implementation also carries the
parallel writers' in-flight library changes; this note distinguishes the one
new behavioral Red from tests that were already green when reviewed.

## Public seams

- **S2 core:** `zotero_list`, `zotero_attachment`, and
  `configure_webdav_with_store` against a temporary Zotero SQLite fixture and
  the real file credential store. No real library or remote server is used.
- **S1 CLI:** the compiled `booxctl` child process, environment-selected file
  store, bounded `TMPDIR`, and exit code/JSON assertions.
- **S3 loopback:** a synthetic WebDAV server accepts one request and holds its
  response headers or body. The CLI is started with `--timeout 300`, so exiting
  within the child watchdog demonstrates interruption of the held request
  rather than expiry of the user timeout.

## Behavioral Red

The list projection claimed a missing attachment was downloadable whenever a
credential record merely loaded. A record that loads but cannot be base64
decoded still fails the actual download before any connection is opened. The
public core regression first failed against the previous production code:

```bash
CARGO_NET_OFFLINE=true cargo test --locked -p boox-core --test cli_libraries \
  zotero_list_reports_webdav_download_only_when_credentials_are_usable -- --nocapture
```

Log `/tmp/booxctl-library-finalize-red.log`, exit **101**: 0 passed / 1 failed,
at `assert!(!undecodable.can_download_from_webdav)`. After that assertion, the
same test also requires `zotero_attachment` to return the typed
`invalid WebDAV credential` local error.

The two CLI SIGINT cases in `library_cancel.rs` were already passing when this
slice reviewed them; this note does not claim a Red for those cases. They
cover two distinct in-flight points: response headers withheld and response
body withheld.

## Green

```bash
CARGO_NET_OFFLINE=true cargo test --locked -p boox-core --test cli_libraries \
  zotero_list_reports_webdav_download_only_when_credentials_are_usable -- --nocapture
CARGO_NET_OFFLINE=true cargo test --locked -p boox-core --test cli_libraries
CARGO_NET_OFFLINE=true cargo test --locked -p booxctl --test library_cancel
CARGO_NET_OFFLINE=true cargo test --locked -p booxctl --test library_push_e2e
CARGO_NET_OFFLINE=true cargo clippy --locked -p boox-core -p booxctl --all-targets -- -D warnings
```

The focused Red/Green log is `/tmp/booxctl-library-finalize-green.log`. The
22 library core tests include the list projection, local and remote
attachment resolution, malformed and unreadable credentials, 401/404, corrupt
and unsafe ZIPs, the expanded-size limit, a chunked body without
`Content-Length`, Calibre paging, and locked databases. The CLI SIGINT cases
assert exit **130**, `error.code == "cancelled"`, no upload request, unchanged
`zotero.sqlite`, and no `booxctl-attachment-*` directory under the controlled
`TMPDIR`. The loopback push tests cover successful download→push→cleanup and
cleanup when submission is unknown.

## Limits

- Cancellation covers waiting for WebDAV response headers and body chunks. ZIP
  parsing and local extraction read only the bounded in-memory archive and are
  not interrupted mid-operation; cancellation is checked after the body and
  again by the shared push pipeline before upload.
- A blocking filesystem, DNS, or connect syscall may not return immediately.
  The poller can stop waiting, but the note does not claim the kernel call
  itself was aborted.
- The list projection reports whether a stored credential can be loaded and
  decoded and a remote download attempted. It does not contact WebDAV, so it
  cannot prove the URL, password, or remote object is valid now.
- Windows Ctrl-C and real platform Credential Manager/keychain behavior remain
  unverified. Linux uses the file store in these tests.
- No real Zotero library, private book, account, or WebDAV service was used.
  The documented manual 401/404 and remote attachment acceptance gates remain
  open.
