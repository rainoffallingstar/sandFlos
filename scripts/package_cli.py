#!/usr/bin/env python3
"""Build the standalone CLI distribution directory that CI publishes."""

from __future__ import annotations

import argparse
import hashlib
import os
import shutil
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent


def binary_name() -> str:
    return "sandflos.exe" if os.name == "nt" else "sandflos"


def file_digest(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Package the release CLI binary with its checksum and README."
    )
    parser.add_argument(
        "--binary",
        type=Path,
        default=REPO_ROOT / "target" / "release" / binary_name(),
        help="release binary to package (default: target/release/sandflos[.exe])",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=REPO_ROOT / "cli-package",
        help="destination directory (default: ./cli-package)",
    )
    parser.add_argument(
        "--readme",
        type=Path,
        default=REPO_ROOT / "docs" / "cli.md",
        help="README copied into the package (default: docs/cli.md)",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="replace an existing output directory",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    output = args.output
    replace_output = output.exists()
    if replace_output:
        if not args.force:
            raise SystemExit(
                f"output already exists: {output} (use --force to replace it)"
            )
        resolved = output.resolve()
        protected = {Path(resolved.anchor), REPO_ROOT.resolve(), Path.cwd().resolve()}
        if resolved in protected or resolved in REPO_ROOT.resolve().parents:
            raise SystemExit(f"refusing to replace protected directory: {output}")

    if not args.binary.is_file():
        raise SystemExit(f"release binary not found: {args.binary}")
    if args.binary.name != binary_name():
        raise SystemExit(
            f"unexpected binary name {args.binary.name!r}; expected {binary_name()!r}"
        )
    if not args.readme.is_file():
        raise SystemExit(f"package README not found: {args.readme}")

    if replace_output:
        allowed = {binary_name(), "README.md", "SHA256SUMS"}
        if output.is_symlink() or not output.is_dir() or any(
            p.is_symlink() or not p.is_file() or p.name not in allowed
            for p in output.iterdir()
        ):
            raise SystemExit("refusing to replace a directory containing unrelated files")
        if (output.resolve() in args.binary.resolve().parents
                or output.resolve() in args.readme.resolve().parents):
            raise SystemExit("refusing to replace an input directory")
        shutil.rmtree(output)
    output.mkdir(parents=True)
    packaged_binary = output / args.binary.name
    shutil.copy2(args.binary, packaged_binary)
    if os.name != "nt":
        packaged_binary.chmod(0o755)
    shutil.copy2(args.readme, output / "README.md")

    checksum_lines = []
    for name in (packaged_binary.name, "README.md"):
        checksum_lines.append(f"{file_digest(output / name)}  {name}")
    (output / "SHA256SUMS").write_text("\n".join(checksum_lines) + "\n", encoding="utf-8")

    print(f"Packaged {args.binary} into {output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
