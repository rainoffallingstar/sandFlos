# Browser login delivery and owned portal lifetime (S3)

Date: 2026-10-30. Base: `3cc6fb7f29167bd722487c5fde0008c29f3f63ae`. Scope is the public browser-login delivery seam in `crates/core/src/auth.rs`, its public loopback test `crates/core/tests/login_delivery.rs`, and this record. No desktop migration, global-state removal, manifest change, or real account request was made.

## Behavioral Red

The first implementation round accepted a delivery callback but ignored it and kept writing global auth directly. The public loopback test established that behavior before implementation:

```bash
CARGO_NET_OFFLINE=true cargo test -p boox-core --test login_delivery -- --nocapture --test-threads=1
```

- `delivery_is_explicit_retryable_once_only_and_portal_shutdown_is_bounded` failed with `body: {"ok":true}`: a persistence callback failure was reported as success and no delivered token was recorded.
- `opener_failure_returns_an_error_and_stops_the_portal` failed because the listener survived startup failure.

Review then found that shutdown still detached the worker after a one-second wait, and that `start_login_flow` removed the owned portal with `std::mem::forget`. Two public tests were added before changing those paths:

- `dropping_with_partial_request_joins_worker_and_suppresses_late_delivery` opens a real loopback connection and sends only the first line of a token callback request. It drops the portal, then finishes the request. Before the fix it failed with `left: 1, right: 0`: the detached worker read the rest of the request after drop and invoked the callback.
- `legacy_login_replaces_its_previous_owned_portal` starts the legacy flow twice, then checks the first port. Before the fix it failed with `starting login again must stop the previously held portal`.

Those are behavioral failures through the public API after a successful compile. Missing APIs or compile errors were not counted as Red.

## Implementation

- `LoginPortal::start` uses production endpoints. `start_with_endpoints` accepts explicit `ApiEndpoints` for alternate deployments and loopback tests.
- Confirmed BOOX QR polling, the WeChat callback, and phone/email submission all use one `TokenDelivery` gate. A token is delivered at most once after a successful callback. A callback error remains retryable. Missing tokens and invalid state are rejected before delivery. Callback error text is discarded, so the browser receives only a category message.
- Portal-owned reads use a 50 ms socket read timeout and poll the stop flag between reads. The whole request head has a fixed two-second deadline. Response writes use a 250 ms timeout. These bounds apply to request parsing and portal responses only.
- Drop sets stop under the delivery gate and then joins the actual worker. No worker or listener is detached. A callback that has already started may finish; its public contract requires a bounded return. After stop is recorded, a waiting or later delivery attempt is rejected. The worker checks stop after accept, after parsing, and in the delivery gate.
- `start_login_flow` remains compatible: it writes the legacy global auth token and portal metadata. The portal is now held in a private `OnceLock<Mutex<Option<LoginPortal>>>` in `auth.rs`. Starting legacy login again replaces that holder and drops, hence joins, the previous portal before publishing the new global metadata. There is no `std::mem::forget`.
- App-owned portals keep using only the supplied callback. They do not read or write global auth, the default credential cache, or the CLI's files.

Status-first response parsing, request construction, endpoint injection, API error categories, and the existing browser HTML and behavior remain intact.

## Green and evidence

The focused public delivery and existing QR failure tests pass together:

```bash
CARGO_NET_OFFLINE=true cargo test -p boox-core --test login_delivery \
  --test auth_qr_failures -- --test-threads=1
```

Result: **9 passed, 0 failed** (6 delivery, 3 QR failure). The delivery coverage is:

- invalid state (`400`) with no callback invocation;
- empty token rejected without delivery;
- callback persistence failure reported as `ok:false`, with no token or callback error text in the response and no global token;
- retry of the same synthetic token after failure, then duplicate success with exactly two callback attempts and one success;
- global auth remaining empty throughout app-owned delivery;
- bounded `shutdown()` and refusal of new connections;
- drop stopping the listener;
- opener failure returning the original error and leaving no listener;
- confirmed QR polling and email submission reaching the same callback through a local upstream fixture;
- drop while a callback request is only partially written, followed by completion of that request, with zero late callbacks and a closed listener;
- replacement legacy login closing the prior listener while leaving the new portal available.

Focused strict clippy covers the library and delivery test:

```bash
CARGO_NET_OFFLINE=true cargo clippy -p boox-core --lib --test login_delivery -- -D warnings
```

The targeted tests use loopback sockets, synthetic tokens, and fake upstream responses. No production login service, account, device, user library, or real credential was contacted.

## Remaining limits

- The worker is blocking and handles one request at a time. QR create/check, WeChat callback, phone/email submission, and the browser page are still subject to the existing 20-second upstream HTTP timeout. Shutdown joins the worker, so if an upstream request is already in progress, shutdown can wait up to that remaining timeout. The portal's own reads, writes, and partial request heads are bounded independently.
- The delivery callback runs synchronously on the portal worker. An application callback that blocks indefinitely cannot be forcibly interrupted in safe Rust, so it delays the join. The public struct documentation requires a short bounded callback and forbids waiting on unbounded persistence or network work there.
- The public seam delivers a token to the application callback; it does not perform app persistence, credential import, refresh, or dashboard refresh. `start_login_flow` keeps the old global behavior only as a compatibility adapter.
- No real WeChat WebSocket, AliCloud captcha, browser rendering, keychain, or production upstream behavior was exercised. Those remain manual/platform acceptance items.
- The public tests use `ApiEndpoints::loopback`; endpoint validation requires a literal loopback origin and does not permit a general test override.
- No desktop app migration was made. `crates/core/src/state.rs`, `crates/core/src/services.rs`, and manifests were left to their owners.
- This work was not committed, and no full workspace suite or shared-file formatting was run while other writers were active.
