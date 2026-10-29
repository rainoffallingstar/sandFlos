#!/usr/bin/env python3
"""Verify a downloaded sandflos CLI package and run its binary without a display."""

from __future__ import annotations

import argparse
import hashlib
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

SUM_FILE = "SHA256SUMS"
README = "README.md"


def binary_name() -> str:
    return "sandflos.exe" if os.name == "nt" else "sandflos"


def file_digest(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def safe_package_path(name: str) -> Path:
    if not name or "\x00" in name:
        raise SystemExit(f"unsafe package path: {name!r}")
    normalized = name.replace("\\", "/")
    if normalized.startswith("/"):
        raise SystemExit(f"unsafe package path: {name!r}")
    parts = [part for part in normalized.split("/") if part not in ("", ".")]
    if not parts or ".." in parts or any(":" in part for part in parts):
        raise SystemExit(f"unsafe package path: {name!r}")
    return Path(*parts)


def read_checksums(path: Path) -> dict[str, str]:
    entries: dict[str, str] = {}
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise SystemExit(f"cannot read {path}: {error}") from error
    for number, line in enumerate(lines, 1):
        if not line.strip():
            continue
        fields = line.split(None, 1)
        if len(fields) != 2:
            raise SystemExit(f"{path}:{number}: expected '<sha256>  <name>'")
        digest, name = fields
        name = name.lstrip("*")
        if len(digest) != 64 or any(ch not in "0123456789abcdefABCDEF" for ch in digest):
            raise SystemExit(f"{path}:{number}: invalid SHA-256 digest")
        normalized = safe_package_path(name).as_posix()
        if normalized in entries:
            raise SystemExit(f"{path}:{number}: duplicate entry {name!r}")
        entries[normalized] = digest.lower()
    if not entries:
        raise SystemExit(f"{path}: no checksum entries")
    return entries


def extract_zip(archive: Path, destination: Path) -> None:
    seen: set[str] = set()
    with zipfile.ZipFile(archive) as bundle:
        for info in bundle.infolist():
            if info.is_dir():
                continue
            relative = safe_package_path(info.filename)
            normalized = relative.as_posix()
            if normalized in seen:
                raise SystemExit(f"duplicate package entry: {info.filename!r}")
            seen.add(normalized)
            mode = (info.external_attr >> 16) & 0xFFFF
            if stat.S_ISLNK(mode):
                raise SystemExit(f"symbolic link in package: {info.filename!r}")
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            with bundle.open(info) as source, target.open("wb") as output:
                shutil.copyfileobj(source, output)
            if os.name != "nt" and (permissions := stat.S_IMODE(mode)):
                target.chmod(permissions)


def verify(root: Path) -> Path:
    root = root.resolve()
    if not root.is_dir():
        raise SystemExit(f"package directory not found: {root}")
    sums_path = root / SUM_FILE
    readme_path = root / README
    if not sums_path.is_file():
        raise SystemExit(f"package is missing {SUM_FILE}")
    if not readme_path.is_file() or readme_path.stat().st_size == 0:
        raise SystemExit(f"package is missing a non-empty {README}")

    entries = read_checksums(sums_path)
    missing = {binary_name()} - entries.keys()
    if missing:
        raise SystemExit(f"checksum file does not cover: {sorted(missing)}")

    actual = {
        path.relative_to(root).as_posix()
        for path in root.rglob("*")
        if path.is_file()
    }
    expected = set(entries) | {SUM_FILE, README}
    if actual != expected:
        raise SystemExit(
            "package contents do not match checksums; "
            f"unexpected={sorted(actual - expected)}, missing={sorted(expected - actual)}"
        )

    for name, expected_digest in entries.items():
        path = root / safe_package_path(name)
        if not path.is_file():
            raise SystemExit(f"checksum entry has no file: {name}")
        if file_digest(path) != expected_digest:
            raise SystemExit(f"SHA-256 mismatch: {name}")

    binary = root / binary_name()
    if os.name != "nt" and not (binary.stat().st_mode & 0o111):
        raise SystemExit(f"packaged binary is not executable: {binary}")

    with tempfile.TemporaryDirectory(prefix="sandflos-run-") as run_dir:
        env = dict(os.environ)
        for key in (
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "SANDFLOS_TOKEN",
            "SANDFLOS_TEST_AUTH_BASE_URL",
        ):
            env.pop(key, None)
        config = Path(run_dir) / "config"
        env["SANDFLOS_CONFIG_DIR"] = str(config)
        for option, needle in (("--help", "usage:"), ("--version", "sandflos")):
            try:
                result = subprocess.run(
                    [str(binary), option],
                    cwd=run_dir,
                    env=env,
                    capture_output=True,
                    text=True,
                    timeout=10,
                )
            except (OSError, subprocess.TimeoutExpired) as error:
                raise SystemExit(f"cannot run packaged binary {option}: {error}") from error
            if result.returncode != 0:
                raise SystemExit(
                    f"packaged binary {option} exited {result.returncode}: "
                    f"{result.stderr.strip()}"
                )
            output = (result.stdout + result.stderr).lower()
            if needle not in output:
                raise SystemExit(
                    f"packaged binary {option} produced unexpected output: "
                    f"{result.stdout.strip() or result.stderr.strip()}"
                )
        if config.exists():
            raise SystemExit("packaged binary created configuration state")

    return binary


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Verify a downloaded sandflos CLI package or artifact ZIP."
    )
    parser.add_argument(
        "package",
        type=Path,
        help="extracted package directory or GitHub Actions artifact ZIP",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    source = args.package
    with tempfile.TemporaryDirectory(prefix="sandflos-package-") as temp:
        if source.is_dir():
            root = source
        elif source.is_file() and zipfile.is_zipfile(source):
            root = Path(temp) / "package"
            root.mkdir()
            extract_zip(source, root)
        else:
            raise SystemExit(f"package is not a directory or ZIP archive: {source}")
        binary = verify(root)

    print(f"Verified CLI package: {binary.name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
