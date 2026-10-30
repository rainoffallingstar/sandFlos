# CLI next standards review

Date: 2026-10-30. Scope: working tree against base `406ef53720dabf1df412359558dc8b8bc2002940`; review only, no production edits.

The async cancellation boundary has sound drop ordering: the request future is pinned and dropped inside the runtime, then the async client is dropped before the runtime (`crates/core/src/services.rs:1334-1349`); body chunks are 64 KiB (`:1176-1177`). I found no blocking standards defect there. The sampled-`VmHWM` caveat and opt-in benchmark scope are documented (`docs/plans/cli-upload-performance.md:3,15`).

1. **Duplicated HTTP client policy (P2, Duplicated Code; judgement).** `HttpPushBackend::upload` rebuilds redirect policy and timeout at `crates/core/src/services.rs:1315-1319` instead of sharing the policy in `create_client` (`crates/core/src/api.rs:184-189`). The existing blocking client is configured at `services.rs:1111` with `timeout.as_secs().max(1)`, while the async client uses raw `self.timeout`; sub-second callers can get different deadlines. Extract one async-client factory or keep a configured async client on the backend.

2. **Stale fixture explanation (P2, Mysterious Name/comment; judgement).** `crates/cli/tests/slow_cancel_backpressure.rs:1-6` says a reqwest worker thread is blocked and never asks for another chunk. The implementation now uses a current-thread runtime and selects the request future. Update the comment and rename `ProbeFixture` (`:32,:41`) so future cancellation work is not misled.

3. **Duplicated cancellation fixture (P2, Duplicated Code; judgement).** The protocol server and `fixture_response` in `slow_cancel_backpressure.rs:180-271` duplicate `crates/cli/tests/push_cancellation.rs:288-383`. A protocol change must now be applied twice. Extract a shared loopback fixture helper or extend the existing slow-upload fixture.

4. **Best-effort fixture-thread cleanup (P3, cleanup risk; judgement).** `slow_cancel_backpressure.rs:164-177` joins only when the thread finishes within two seconds; otherwise the accept loop remains alive. Make timeout failure explicit or close the listener so a failed test cannot retain a loopback thread.

5. **Benchmark child pipes and error state (P2/P3, robustness).** `scripts/benchmark_cli_upload.py:278-303` samples `/proc` before draining the child's stdout/stderr; added progress output can fill a pipe and stall the measured process. Drain both concurrently or use temporary files. Also serialize `FixtureServer.errors` appends (`:109-110,:228-233`) and read them under the same lock so failure diagnostics cannot race the verify step.

## Parent dispositions

1. Retained as a small policy duplication: both clients reject redirects and use the requested timeout. The OSS adapter retains the public Duration precision; legacy metadata calls still round to whole seconds. CLI --timeout accepts whole seconds, so its behavior agrees. A shared factory for two builder options would add an unused policy abstraction; sub-second consistency across all core adapters remains a separate cleanup.
2. Fixed: fixture renamed SlowUploadFixture; comment describes the pending async request.
3. Retained judgement: the new fixture deliberately stalls after a prefix; the old fixture drains/acknowledges or drops a submission response. Shared protocol samples remain duplicated; no generic server framework was added in this cancellation fix.
4. Fixed: normal fixture cleanup now fails explicitly if the thread misses its two-second bound or panics; during panic unwinding the stop flag and finite I/O bounds remain active without causing a second panic.
5. Fixed: benchmark stdout/stderr go to temporary files, preventing pipe backpressure; fixture diagnostics use the same state lock. A finally block kills/reaps the child on interruption. Only the CLI child is sampled, as documented.

No hard documented-standard violation remained. These judgement dispositions do not imply broader architecture migration is complete.

Parent integration followup: the stalled fixture previously used one read() for its prefix but required at least 4 KiB in the test. TCP may split that prefix across reads. It now read_exacts the bounded 8 KiB prefix (within the existing socket timeout) before stalling; the cancellation and request assertions remain unchanged. This is fixture reliability, not a new production behavior Red.
