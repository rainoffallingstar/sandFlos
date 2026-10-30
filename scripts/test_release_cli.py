"""Release archive acceptance at the S4 process boundary."""
import os
import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import zipfile

SCRIPTS = Path(__file__).resolve().parent
BINARY = 'sandflos.exe' if os.name == 'nt' else 'sandflos'


def run(script, *args):
    return subprocess.run([sys.executable, str(SCRIPTS / script), *map(str, args)],
                          capture_output=True, text=True, timeout=30)


class ReleaseTests(unittest.TestCase):
    def test_tag_must_match_cli_manifest_before_creating_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / 'release'
            result = run('release_cli.py', 'archive', '--tag', 'cli-v99.0.0',
                         '--target', 'x86_64-unknown-linux-gnu', '--package', tmp,
                         '--output-dir', output)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('does not match CLI version', result.stderr)
            self.assertFalse(output.exists())

    @unittest.skipIf(os.name == 'nt', 'native Windows release binary is tested by CI')
    def test_release_zip_runs_and_cannot_replace_an_existing_archive(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = root / BINARY
            binary.write_text('#!/bin/sh\ncase "$1" in\n--help) echo "Usage: sandflos";;\n--version) echo "sandflos test";;\nesac\n')
            binary.chmod(0o755)
            readme = root / 'readme'
            readme.write_text('# CLI\n')
            package = root / 'package'
            result = run('package_cli.py', '--binary', binary, '--readme', readme,
                         '--output', package)
            self.assertEqual(result.returncode, 0, result.stderr)
            args = ('archive', '--tag', 'cli-v0.1.0', '--target',
                    'x86_64-unknown-linux-gnu', '--package', package,
                    '--output-dir', root / 'release')
            result = run('release_cli.py', *args)
            self.assertEqual(result.returncode, 0, result.stderr)
            archive = root / 'release/sandflos-cli-v0.1.0-x86_64-unknown-linux-gnu.zip'
            with zipfile.ZipFile(archive) as bundle:
                self.assertEqual(set(bundle.namelist()), {BINARY, 'README.md', 'SHA256SUMS'})
            result = run('verify_cli_package.py', archive)
            self.assertEqual(result.returncode, 0, result.stderr)
            before = archive.read_bytes()
            self.assertEqual(archive.with_suffix('.zip.sha256').read_text(),
                             f'{hashlib.sha256(before).hexdigest()}  {archive.name}\n')
            result = run('release_cli.py', *args)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(archive.read_bytes(), before)


if __name__ == '__main__':
    unittest.main()
