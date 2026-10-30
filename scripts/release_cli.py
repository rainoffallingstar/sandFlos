#!/usr/bin/env python3
"""Validate CLI release tags and create standalone release archives."""
from __future__ import annotations

import argparse
import hashlib
from pathlib import Path
import subprocess
import sys
import tomllib
import zipfile

REPO_ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    check = commands.add_parser('check-tag')
    check.add_argument('--tag', required=True)
    archive = commands.add_parser('archive')
    archive.add_argument('--tag', required=True)
    archive.add_argument('--target', required=True)
    archive.add_argument('--package', type=Path, required=True)
    archive.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    manifest = tomllib.loads((REPO_ROOT / 'crates/cli/Cargo.toml').read_text(encoding='utf-8'))
    version = manifest['package']['version']
    if args.tag != f'cli-v{version}':
        parser.error(f'tag {args.tag!r} does not match CLI version {version!r}')
    if args.command == 'archive':
        targets = {'x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu',
                   'x86_64-apple-darwin', 'aarch64-apple-darwin',
                   'x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc'}
        if args.target not in targets:
            parser.error('unsupported release target')
        # Execute the actual native binary before creating any release output.
        subprocess.run([sys.executable, str(REPO_ROOT / 'scripts/verify_cli_package.py'),
                        str(args.package)], check=True)
        args.output_dir.mkdir(parents=True, exist_ok=True)
        destination = args.output_dir / f'sandflos-{args.tag}-{args.target}.zip'
        sums = destination.with_suffix('.zip.sha256')
        if destination.exists() or sums.exists():
            parser.error('refusing to replace an existing release archive or checksum')
        binary = 'sandflos.exe' if 'windows' in args.target else 'sandflos'
        created = []
        try:
            with zipfile.ZipFile(destination, 'x', compression=zipfile.ZIP_DEFLATED) as bundle:
                created.append(destination)
                for name in (binary, 'README.md', 'SHA256SUMS'):
                    bundle.write(args.package / name, name)
            with destination.open('rb') as stream:
                digest = hashlib.file_digest(stream, 'sha256').hexdigest()
            with sums.open('x', encoding='utf-8', newline='\n') as stream:
                created.append(sums)
                stream.write(f'{digest}  {destination.name}\n')
        except Exception:
            for path in created:
                path.unlink(missing_ok=True)
            raise
        print(f'Created CLI release archive: {destination}')
        return 0
    print(f'Validated CLI release tag: {args.tag}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
