"""Acceptance tests at the packaging CLI boundary (S4)."""
import hashlib
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest
import warnings
import zipfile

SCRIPTS = Path(__file__).resolve().parent
BINARY = 'booxctl.exe' if os.name == 'nt' else 'booxctl'


def run(script, *args, cwd=None):
    return subprocess.run([sys.executable, str(SCRIPTS / script), *map(str, args)],
                          cwd=cwd, capture_output=True, text=True, timeout=20)


def fixture(root):
    root.mkdir()
    # Invalid executable; rejection cases must fail before attempting to run it.
    (root / BINARY).write_bytes(b'not executable')
    (root / BINARY).chmod(0o755)
    (root / 'README.md').write_bytes(b'# CLI\n')
    checksum(root)


def checksum(root):
    names = [BINARY, 'README.md']
    (root / 'SHA256SUMS').write_text(''.join(
        f'{hashlib.sha256((root / n).read_bytes()).hexdigest()}  {n}\n' for n in names))


class PackageTests(unittest.TestCase):
    def test_unsafe_zip_members_are_rejected(self):
        for name in ('../escape', '/absolute', '..\\escape', 'C:\\escape', 'extra.txt'):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as tmp:
                archive = Path(tmp) / 'bad.zip'
                with zipfile.ZipFile(archive, 'w') as z:
                    z.writestr(name, b'x')
                result = run('verify_cli_package.py', archive)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn('cannot run packaged binary', result.stderr)

    def test_duplicate_and_symlink_zip_members_are_rejected(self):
        for duplicate in (True, False):
            with self.subTest(duplicate=duplicate), tempfile.TemporaryDirectory() as tmp:
                archive = Path(tmp) / 'bad.zip'
                with warnings.catch_warnings():
                    warnings.simplefilter('ignore', UserWarning)
                    with zipfile.ZipFile(archive, 'w') as z:
                        info = zipfile.ZipInfo(BINARY)
                        info.external_attr = ((stat.S_IFREG if duplicate else stat.S_IFLNK) | 0o755) << 16
                        z.writestr(info, b'x')
                        if duplicate:
                            z.writestr(BINARY, b'y')
                result = run('verify_cli_package.py', archive)
                self.assertNotEqual(result.returncode, 0)
                self.assertRegex(result.stderr, 'duplicate|symbolic link')

    def test_tampering_missing_and_checksummed_extra_files_are_rejected(self):
        for case in ('tamper', 'missing', 'extra', 'bad-checksum'):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp) / 'package'
                fixture(root)
                if case == 'tamper':
                    (root / 'README.md').write_text('changed')
                elif case == 'missing':
                    (root / 'SHA256SUMS').unlink()
                elif case == 'extra':
                    (root / 'extra.txt').write_text('extra')
                    with (root / 'SHA256SUMS').open('a') as f:
                        f.write(f'{hashlib.sha256(b"extra").hexdigest()}  extra.txt\n')
                else:
                    (root / 'SHA256SUMS').write_text('bad digest\n')
                result = run('verify_cli_package.py', root)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn('cannot run packaged binary', result.stderr)

    def test_force_preserves_unrelated_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = root / BINARY
            binary.write_bytes(b'x')
            readme = root / 'readme'
            readme.write_bytes(b'# CLI')
            output = root / 'output'
            output.mkdir()
            sentinel = output / 'keep.txt'
            sentinel.write_bytes(b'keep')
            result = run('package_cli.py', '--binary', binary, '--readme', readme,
                         '--output', output, '--force')
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(sentinel.read_bytes(), b'keep')

    @unittest.skipIf(os.name == 'nt', 'POSIX fixture; CI separately runs the native release binary')
    def test_package_runs_after_zip_download_and_relative_directory(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = root / BINARY
            binary.write_text('#!/bin/sh\ncase "$1" in\n--help) echo "Usage: booxctl";;\n--version) echo "booxctl test";;\nesac\n')
            binary.chmod(0o755)
            readme = root / 'readme'
            readme.write_text('# CLI\n')
            result = run('package_cli.py', '--binary', binary, '--readme', readme,
                         '--output', root / 'package')
            self.assertEqual(result.returncode, 0, result.stderr)
            result = run('verify_cli_package.py', 'package', cwd=root)
            self.assertEqual(result.returncode, 0, result.stderr)
            archive = root / 'download.zip'
            with zipfile.ZipFile(archive, 'w') as z:
                for path in (root / 'package').iterdir():
                    z.write(path, path.name)
            result = run('verify_cli_package.py', archive)
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
