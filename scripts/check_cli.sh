#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
python3 -m unittest discover -s scripts -p 'test_*.py'
cargo fmt -p boox-core -p sandflos-cli -- --check
cargo test --locked -p boox-core -p sandflos-cli
cargo clippy --locked -p boox-core -p sandflos-cli --all-targets -- -D warnings
cargo build --locked --release -p sandflos-cli
python3 scripts/check_cli_artifact.py
python3 scripts/package_cli.py --output target/cli-package --force
python3 scripts/verify_cli_package.py target/cli-package
