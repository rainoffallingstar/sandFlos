from __future__ import annotations

import os
import stat
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

import verify_cli_package as verifier

SCRIPTS = Path(__file__).resolve().parent


def write_binary(path: Path) -> bytes:
    contents = b"#!/bin/sh\ncase \"$1\" in\n  --help) echo 'Usage: sandflos' ;;\n  --version) echo 'sandflos test' ;;\nesac\n"
    path.write_bytes(contents)
    if os.name != "nt":
        path.chmod(0o755)
    return contents


def make_package(root: Path) -> Path:
    root.mkdir()
    binary = root / verifier.binary_name()
    write_binary(binary)
    readme = root / "README.md"
    readme.write_text("# CLI\n", encoding="utf-8")
    (root / "SHA256SUMS").write_text(
        f"{verifier.file_digest(binary)}  {binary.name}\n"
        f"{verifier.file_digest(readme)}  README.md\n",
        encoding="utf-8",
    )
    return binary


class PackageVerifierTests(unittest.TestCase):
    def test_rejects_unsafe_package_paths(self) -> None:
        for name in (
            "../escape",
            "/absolute",
            "a/../../escape",
            "..\\escape",
            "C:\\absolute\\file",
            "",
            "a\x00b",
            "dir/file:stream",
        ):
            with self.subTest(name=name):
                with self.assertRaises(SystemExit):
                    verifier.safe_package_path(name)

    def test_checksum_reader_rejects_bad_entries(self) -> None:
        cases = {
            "bad digest": "not-a-digest  sandflos\n",
            "traversal": f"{'0' * 64}  ../sandflos\n",
            "duplicate": f"{'0' * 64}  sandflos\n{'1' * 64}  sandflos\n",
            "normalized duplicate": f"{'0' * 64}  sandflos\n{'1' * 64}  ./sandflos\n",
            "empty": "\n",
        }
        for label, contents in cases.items():
            with self.subTest(label=label), tempfile.TemporaryDirectory() as temp:
                path = Path(temp) / "SHA256SUMS"
                path.write_text(contents, encoding="utf-8")
                with self.assertRaises(SystemExit):
                    verifier.read_checksums(path)

    def test_extraction_rejects_traversal_and_symlinks(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            temp = Path(temp)
            traversal = temp / "traversal.zip"
            with zipfile.ZipFile(traversal, "w") as bundle:
                bundle.writestr("../escape", b"x")
            destination = temp / "out"
            destination.mkdir()
            with self.assertRaises(SystemExit):
                verifier.extract_zip(traversal, destination)

            symlink = temp / "symlink.zip"
            with zipfile.ZipFile(symlink, "w") as bundle:
                info = zipfile.ZipInfo("sandflos")
                info.external_attr = (stat.S_IFLNK | 0o777) << 16
                bundle.writestr(info, "README.md")
            with self.assertRaises(SystemExit):
                verifier.extract_zip(symlink, destination)

            duplicate = temp / "duplicate.zip"
            with zipfile.ZipFile(duplicate, "w") as bundle:
                bundle.writestr("sandflos", b"one")
                bundle.writestr("./sandflos", b"two")
            with self.assertRaises(SystemExit):
                verifier.extract_zip(duplicate, destination)

    def test_verify_rejects_missing_extra_and_tampered_files(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "package"
            make_package(root)
            (root / "extra.txt").write_text("unexpected", encoding="utf-8")
            with self.assertRaises(SystemExit):
                verifier.verify(root)

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "package"
            make_package(root)
            (root / "README.md").write_text("# changed\n", encoding="utf-8")
            with self.assertRaises(SystemExit):
                verifier.verify(root)

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "package"
            make_package(root)
            (root / "SHA256SUMS").unlink()
            with self.assertRaises(SystemExit):
                verifier.verify(root)

    def test_package_cli_refuses_to_replace_the_repository(self) -> None:
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPTS / "package_cli.py"),
                "--output",
                str(SCRIPTS.parent),
                "--force",
            ],
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("refusing to replace protected directory", result.stderr)

    @unittest.skipIf(os.name == "nt", "the fixture binary is a POSIX shell script")
    def test_package_cli_produces_a_runnable_package(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            temp = Path(temp)
            binary = temp / verifier.binary_name()
            write_binary(binary)
            readme = temp / "README.md"
            readme.write_text("# CLI\n", encoding="utf-8")
            output = temp / "package"
            subprocess.run(
                [
                    sys.executable,
                    str(SCRIPTS / "package_cli.py"),
                    "--binary",
                    str(binary),
                    "--readme",
                    str(readme),
                    "--output",
                    str(output),
                ],
                check=True,
                capture_output=True,
                text=True,
            )
            verified = verifier.verify(output)
            self.assertEqual(verified.name, verifier.binary_name())

    @unittest.skipIf(os.name == "nt", "the fixture binary is a POSIX shell script")
    def test_relative_package_directory_runs_binary(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            temp = Path(temp)
            root = temp / "package"
            make_package(root)
            previous = Path.cwd()
            try:
                os.chdir(root.parent)
                verified = verifier.verify(Path("package"))
            finally:
                os.chdir(previous)
            self.assertTrue(verified.is_absolute())


if __name__ == "__main__":
    unittest.main()
