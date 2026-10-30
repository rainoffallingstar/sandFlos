# CLI upload performance evidence

Date: 2026-10-30. Scope: `scripts/benchmark_cli_upload.py` and this document. Base commit `406ef53720dabf1df412359558dc8b8bc2002940`. No production Rust was changed. The large benchmark is standalone and opt-in; it is not wired into `scripts/check_cli.sh` or CI.

## What was measured

The primary path starts the release `sandflos --json push FILE` process with synthetic credentials and literal loopback API/OSS origins. The fixture follows the protocol shapes in `crates/cli/tests/common/mod.rs`: account and storage lookup, bucket and STS responses, the neocloud sync token, the OSS PUT, the message PUT, sync confirmation, and `saveAndPush`. It streams each OSS request body into SHA-256 rather than retaining it. Each accepted fixture connection has a finite 60-second socket read deadline. Every successful run verifies:

- exactly one OSS PUT and one declared `Content-Length`;
- received byte count equal to the source file size;
- received SHA-256 equal to the SHA-256 calculated while generating the file;
- `Content-Type: application/pdf`;
- for the CLI path, one message PUT, one `saveAndPush`, and a JSON file result with `outcome: accepted`, `stage: save_and_push`, and the expected size.

Memory is the benchmark child’s `VmHWM` read from `/proc/<pid>/status` about once per millisecond until the child exits. It excludes the Python fixture, cargo, and the shell. `VmHWM` is the kernel’s high-water mark, but this is still a sampled value: the benchmark can miss a peak that occurs inside the final sampling interval before the process exits. Elapsed time is also for the child process. “Fixture upload throughput” measures from the first OSS request body byte through the last byte; “wall throughput” divides the payload size by the whole child runtime, including account lookup, subsequent metadata requests, and process startup.

The deliberately buffered control is a separate Python child that reads the complete 200 MiB file into memory before sending one PUT. It validates the same received byte count, length, hash, and single PUT, but does not run the message/submission stages. It makes the size sensitivity of the RSS measurement visible. It is not a measurement of this project’s production upload implementation.

## Commands

Build the release CLI and run the opt-in benchmark on Linux:

```bash
cargo build --locked --release -p sandflos-cli
python3 scripts/benchmark_cli_upload.py \
  --runs 1 --output /tmp/sandflos-cli-upload-benchmark-final.json --json
```

The harness writes generated files beneath a fresh temporary directory and removes them with that directory. It does not access real credentials, accounts, books, OSS, or devices; all endpoint responses and credentials are synthetic. The benchmark command itself is the opt-in performance run. Python unittest discovery in `scripts/check_cli.sh` only runs existing `test_*.py` files; this benchmark is not invoked by that check.

## Environment

| Item | Value |
| --- | --- |
| Host | Linux (WSL2), x86_64 |
| CPU | AMD Ryzen 9 5900HX, 16 logical CPUs |
| Memory | 53,932,048,384 bytes (`MemTotal`) |
| Python | 3.14.8 |
| Rust / Cargo | 1.97.1 / 1.97.1 |
| Source | `406ef53720dabf1df412359558dc8b8bc2002940` |
| Binary | `target/release/sandflos`, 8,886,880 bytes |
| Binary SHA-256 | `5cac15225bfc784140cf282c8debb8e1a52888213b71b7de709b9c470d4914e9` |
| Endpoint | Loopback HTTP fixture on an ephemeral port |

## Results

One CLI run was made at each size, followed by one buffered control. Every run passed the byte-count, SHA-256, Content-Length, Content-Type, single-PUT, and CLI JSON report checks. Each CLI trace also contained one OSS PUT, one message PUT, one `saveAndPush`, and the expected supporting GETs.

| Path | Size | Run | Wall s | OSS body s | OSS MiB/s | Wall MiB/s | Sampled VmHWM MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| CLI streaming | 8 MiB | 1 | 0.427132 | 0.091692 | 87.2 | 18.7 | 5.762 |
| CLI streaming | 200 MiB | 1 | 2.850680 | 2.279153 | 87.8 | 70.2 | 5.938 |
| Buffered Python control | 200 MiB | 1 | 1.298180 | 0.328345 | 609.1 | 154.1 | 226.004 |

The streaming CLI’s sampled peak rose by about 0.18 MiB while the payload increased from 8 to 200 MiB. The buffered control’s sampled peak was 226.004 MiB, about 220.1 MiB above the 200 MiB streaming peak, consistent with holding the payload in memory plus interpreter and transport overhead. These two observations make the flat streaming result meaningful for this run. They do not turn the sampled value into a proof that no shorter-lived peak occurred inside the final sampling interval.

The complete JSON result, including each request trace, CLI report, hash, and sample count, was written to `/tmp/sandflos-cli-upload-benchmark-final.json` during this run. That temporary file is not committed; the harness reproduces the same schema and verification.


## Parent review dispositions

- Removed `scripts/test_benchmark_cli_upload.py` entirely. Its private-helper assertions were outside the agreed public seams and included a tautological payload comparison. The real CLI/HTTP benchmark retains the meaningful end-to-end checks.
- Added a finite 60-second read timeout to every accepted fixture connection before request body reads. The child watchdog still kills and reaps a stuck process.
- Renamed the memory result to sampled `VmHWM` and documented that a peak inside the final one-millisecond sampling interval can be missed.
- Omitted the redacted kernel version rather than fabricating a host kernel value.
- Corrected the documentation: only the large benchmark is opt-in, while Python discovery in `scripts/check_cli.sh` still runs the repository's remaining `test_*.py` files.
- Replaced the placeholder bind with an actual literal loopback address and reduced the harness from 845 to 558 lines by sharing case assembly, removing duplicate summary rendering, and fixing the benchmark to the requested 8 MiB, 200 MiB, and 200 MiB control cases.

## Interpretation and limits

These numbers show the tested release CLI path accepted the exact bytes with a low sampled peak RSS in this Linux environment. They do not establish production network throughput, production OSS behavior, BOOX device delivery, or performance on macOS/Windows. The fixture runs on the same host and has no simulated WAN latency, packet loss, proxy, or server-side throttling. Throughput varies with fixture scheduling and host conditions; the 8 MiB sample is short enough that startup and metadata requests dominate wall time, while the single 200 MiB run is not a statistical throughput estimate.

The measurement covers only the CLI process. It does not measure memory in a desktop parent, a future app-owned runtime state, or any server. The 200 MiB payload is exactly the documented single-file limit, not a claim that arbitrary larger files are accepted.

The buffered control is implemented in Python, so its RSS overhead differs from a Rust buffered implementation. Its purpose is a size-sensitive control for the measurement method. It should not be used to estimate production throughput or memory.

The benchmark is intentionally outside routine CI because repeated 200 MiB runs and process sampling would add resource cost to every validation. Run it explicitly when validating a release build or a transfer change.

## Integrated cancellation build

The parent reran the final harness after rebuilding the OSS cancellation implementation. Command: `python3 scripts/benchmark_cli_upload.py --runs 1 --output /tmp/sandflos-cli-upload-benchmark-integrated.json`. The temporary-file output capture and finally-based child cleanup were active. All cases again passed exact byte/hash/length and single-PUT verification.

| Path | MiB | Wall seconds | OSS MiB/s | Sampled VmHWM MiB |
| --- | ---: | ---: | ---: | ---: |
| streaming_cli | 8 | 0.488904 | 76.6 | 6.137 |
| streaming_cli | 200 | 0.216997 | 1175.8 | 6.156 |
| buffered_python_control | 200 | 1.423887 | 344.0 | 225.746 |

Integrated release binary SHA-256: `1aed29734443435f8f9ccd6029514350e4b0407e280ad6987899bfe7a26603e0`. These single-run values retain all sampling and loopback limits above; throughput variation is not a production speedup claim.
