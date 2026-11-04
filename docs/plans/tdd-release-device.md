# Device release fixes — 2026-11-04

ARP-only candidates and ARP precedence over stale cloud addresses are covered through the public device projection (`device_contract`). Earlier private-helper Red is supplemental evidence, not a new public-seam Red. Public stale-address Red/Green: `/tmp/booxctl-device-finish-{red,green}.log` (101 then 0).

External arp/interface commands now use a two-second deadline and 1 MiB per-stream capture limit. Parent review replaced pipe readers with private temporary capture files, avoiding joins blocked by inherited pipes; timeout kills and reaps the direct child, and temporary files are cleaned on return. File-size polling bounds capture at observation points, not disk bytes written between polls. OS kill/wait/filesystem calls can still block; descendant process trees are not managed.

Command behavior Red: `CARGO_BUILD_JOBS=2 cargo test --locked -p boox-core --lib command_output -- --nocapture`, exit 101, `/tmp/booxctl-device-command-red.log`: timeout and output-limit assertions failed. These temporary probes called the private adapter and were removed after review to keep committed new tests at agreed public seams; archived at `/tmp/booxctl-device-adapter-probes.rs`. Green probe log `/tmp/booxctl-device-command-tests-green.log` reports 9 device tests passed (exit 0). Public device projection coverage is retained. Final parent unified check is recorded in the release execution record.

No real LAN/device/account operations were used. Candidate HTTP addresses use the existing default port convention, not a verified live service. Real device reachability and receipt remain manual gates.
