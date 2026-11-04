# Queue and WebDAV release fixes — 2026-11-04

Inherited queue exit-code and WebDAV availability changes had no independently verified Red log; they are retained and regression-tested, not retrospectively labelled test-first. Parsed command metadata restricts late SIGINT success/partial exceptions to queue; push retains exit 130. A configured readable WebDAV credential enables the offline list capability projection, without claiming the remote attachment exists.

WebDAV uses the existing owned cancellable runtime for headers and body. Chunk reads check the 200 MiB limit before extending the buffer, including responses without Content-Length. ZIP validation/extraction and temporary cleanup remain in place. Cancellation maps to stable `cancelled`, skips upload, and preserves the source database. Password stdin reuses the auth cancellation reader and checks cancellation before configuration persistence.

Evidence:
- `/tmp/library-cancel-red.log`: actual JSON assertion failure (`library_error` versus `cancelled`), exit 101. This proves the error-mapping gap; it does not prove that download cancellation itself was test-first.
- `/tmp/library-cancel-green3.log`: one cancellation regression passed, exit 0.
- `/tmp/booxctl-library-finalize-regression.log`: final targeted `cargo test --locked -p booxctl --test library_cancel` (2 passed) and `--test library_push_e2e` (4 passed), exit 0. Fixtures cover stalled headers and body, no upload after cancellation, and temporary cleanup.
- Final full checks and queue/core library counts are recorded in `cli-release-execution.md`; intermediate compilation failures caused by concurrent edits are not behavioral Red.

All fixtures use synthetic loopback HTTP and temporary SQLite/configuration. No production accounts, libraries, or WebDAV servers. Windows signal handling needs real console acceptance. ZIP contents still reside in memory up to the documented archive limit; filesystem and OS-level calls do not guarantee immediate cancellation.
