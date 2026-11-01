#!/usr/bin/env python3
"""Opt-in Linux loopback upload benchmark through the real booxctl CLI.

Each measured run starts ``booxctl --json push FILE`` with synthetic credentials
and loopback API/OSS endpoints.  The fixture follows the request shapes used by
the CLI transfer tests, streams uploads into SHA-256, and checks exact bytes and
a single PUT.  The optional Python control reads the whole file before one PUT.

Memory is the child's sampled ``VmHWM`` from /proc.  A peak inside the final
sampling interval can be missed.  This is an instrumentation control, not a
claim about production network, OSS, server, or device performance.
"""

from __future__ import annotations

import argparse
import hashlib
import http.client
import json
import os
import platform
import subprocess
import sys
import tempfile
import threading
import time
import traceback
import uuid
from contextlib import contextmanager
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

MIB = 1024 * 1024
SIZES_MIB = (8, 200)
CONTROL_MIB = 200
CHUNK = 256 * 1024
BLOCK = MIB
SEED = b"booxctl-upload-benchmark-v1"
REV = "1-fixture"
SAMPLE_SECONDS = 0.001
SOCKET_TIMEOUT = 60.0
CHILD_TIMEOUT = 300.0
LOOPBACK = "127.0.0.1"


class BenchmarkError(RuntimeError):
    pass


@dataclass
class State:
    lock: threading.Lock = field(default_factory=threading.Lock)
    requests: list[str] = field(default_factory=list)
    puts: int = 0
    received: int = 0
    lengths: list[int] = field(default_factory=list)
    sha256: str = ""
    content_type: str = ""
    started: float = 0.0
    finished: float = 0.0
    message_puts: int = 0
    submissions: int = 0
    message_id: str | None = None

    def add_request(self, line: str) -> None:
        with self.lock:
            self.requests.append(line)

    def begin_put(self, length: int, content_type: str) -> None:
        with self.lock:
            self.puts += 1
            self.started = time.perf_counter()
            self.lengths.append(length)
            self.content_type = content_type

    def finish_put(self, received: int, digest: str) -> None:
        with self.lock:
            self.received = received
            self.sha256 = digest
            self.finished = time.perf_counter()

    def snapshot(self) -> dict:
        with self.lock:
            return {
                "requests": list(self.requests),
                "oss_puts": self.puts,
                "oss_bytes": self.received,
                "oss_lengths": list(self.lengths),
                "oss_sha256": self.sha256,
                "oss_content_type": self.content_type,
                "oss_elapsed_seconds": max(0.0, self.finished - self.started),
                "message_puts": self.message_puts,
                "submissions": self.submissions,
                "message_id": self.message_id,
            }


class FixtureServer(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self) -> None:
        super().__init__((LOOPBACK, 0), Handler)
        self.state = State()
        self.errors: list[str] = []

    def record_error(self) -> None:
        with self.state.lock:
            self.errors.append(traceback.format_exc())

    def error_snapshot(self) -> list[str]:
        with self.state.lock:
            return list(self.errors)

    def handle_error(self, request, client_address) -> None:
        self.record_error()

    @property
    def endpoint(self) -> str:
        return f"http://{LOOPBACK}:{self.server_port}"


def envelope(data: object) -> bytes:
    return json.dumps({"result_code": 0, "message": None, "data": data}).encode()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def setup(self) -> None:
        super().setup()
        self.connection.settimeout(SOCKET_TIMEOUT)

    def log_message(self, format, *args) -> None:
        pass

    def do_GET(self) -> None:
        self.dispatch("GET")

    def do_PUT(self) -> None:
        self.dispatch("PUT")

    def do_POST(self) -> None:
        self.dispatch("POST")

    def length(self) -> int:
        raw = self.headers.get("Content-Length")
        if raw is None:
            raise ValueError("missing Content-Length")
        value = int(raw)
        if value < 0:
            raise ValueError("negative Content-Length")
        return value

    def read_stream(self, length: int) -> tuple[int, str]:
        digest = hashlib.sha256()
        received = 0
        while received < length:
            block = self.rfile.read(min(CHUNK, length - received))
            if not block:
                raise ValueError(f"upload body short by {length - received} bytes")
            digest.update(block)
            received += len(block)
        return received, digest.hexdigest()

    def read_json(self, length: int) -> object:
        raw = self.rfile.read(length)
        if len(raw) != length:
            raise ValueError("short JSON request body")
        return json.loads(raw)

    def dispatch(self, method: str) -> None:
        path = urlsplit(self.path).path
        self.server.state.add_request(f"{method} {path}")
        try:
            if method == "PUT" and path.startswith("/uid-fixture/push/"):
                length = self.length()
                self.server.state.begin_put(
                    length, self.headers.get("Content-Type", "")
                )
                received, digest = self.read_stream(length)
                self.server.state.finish_put(received, digest)
                return self.send(200, b"")
            if method == "GET" and path.startswith("/uid-fixture/push/"):
                return self.send(206, b"", content_type="application/octet-stream")

            length = self.length() if method in {"PUT", "POST"} else 0
            body = self.read_json(length) if length else None

            if method == "GET" and path == "/api/1/users/me":
                return self.send(200, envelope({"uid": "uid-fixture", "nickname": "reader"}))
            if method == "GET" and path == "/api/1/statistics/v2/user/storage":
                return self.send(200, envelope({"usedSize": 0, "totalSize": 1024 * MIB}))
            if method == "GET" and path == "/api/1/config/buckets":
                buckets = {"onyx-cloud": {"bucket": "onyx-cloud", "aliEndpoint": "https://oss-fixture.invalid"}}
                return self.send(200, envelope(buckets))
            if method == "GET" and path == "/api/1/config/stss":
                sts = {
                    "AccessKeyId": "fixture-id",
                    "AccessKeySecret": "fixture-secret",
                    "SecurityToken": "fixture-sts",
                    "Expiration": "2099-01-01T00:00:00Z",
                }
                return self.send(200, envelope(sts))
            if method == "GET" and path == "/api/1/users/syncToken":
                token = envelope({"cookie_name": "SyncGatewaySession", "session_id": "fixture-session"})
                return self.send(200, token, extra=("Set-Cookie: SyncGatewaySession=fixture-session; Path=/",))
            if method == "PUT" and path.startswith("/neocloud/"):
                message_id = path.rsplit("/", 1)[-1]
                if isinstance(body, dict) and isinstance(body.get("_id"), str):
                    message_id = body["_id"]
                with self.server.state.lock:
                    self.server.state.message_puts += 1
                return self.send(201, json.dumps({"id": message_id, "rev": REV}).encode())
            if method == "GET" and path.startswith("/neocloud/"):
                message_id = path.rsplit("/", 1)[-1]
                content = {"formats": ["pdf"], "name": "book.pdf", "size": 0,
                           "storage": {"pdf": {"oss": {"key": "uid-fixture/push/fixture.pdf", "size": 0}}}}
                doc = {"_id": message_id, "_rev": REV, "msgType": 2,
                       "contentType": "digital_content", "name": "book.pdf", "size": 0,
                       "storageKey": "uid-fixture/push/fixture.pdf",
                       "content": json.dumps(content), "updatedAt": 1000}
                return self.send(200, json.dumps(doc).encode())
            if method == "POST" and path == "/api/1/push/saveAndPush":
                message_id = None
                if isinstance(body, dict) and isinstance(body.get("cbMsg"), dict):
                    candidate = body["cbMsg"].get("id")
                    message_id = candidate if isinstance(candidate, str) else None
                with self.server.state.lock:
                    self.server.state.submissions += 1
                    self.server.state.message_id = message_id
                return self.send(200, envelope({"accepted": True}))
            self.send(404, b'{"error":"unexpected fixture request"}')
        except Exception:
            self.server.record_error()
            try:
                self.send(500, b'{"error":"fixture failure"}')
            except Exception:
                self.server.record_error()

    def send(self, status: int, body: bytes, *, content_type: str = "application/json",
             extra: tuple[str, ...] = ()) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        for header in extra:
            self.send_header(*header.split(": ", 1))
        self.end_headers()
        if body:
            self.wfile.write(body)
        self.close_connection = True


@contextmanager
def fixture_server():
    server = FixtureServer()
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield server
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def block(offset: int, size: int) -> bytes:
    return hashlib.shake_256(SEED + offset.to_bytes(8, "big")).digest(size)


def create_payload(path: Path, size: int) -> str:
    digest = hashlib.sha256()
    with path.open("wb") as output:
        for offset in range(0, size, BLOCK):
            chunk = block(offset, min(BLOCK, size - offset))
            output.write(chunk)
            digest.update(chunk)
    return digest.hexdigest()


def run_child(command: list[str], env: dict[str, str]) -> dict:
    started = time.perf_counter()
    with tempfile.TemporaryFile() as out_file, tempfile.TemporaryFile() as err_file:
        process = subprocess.Popen(command, env=env, stdin=subprocess.DEVNULL,
                                   stdout=out_file, stderr=err_file)
        try:
            vmhwm = samples = 0
            while True:
                try:
                    with open(f"/proc/{process.pid}/status", encoding="ascii") as status:
                        for line in status:
                            if line.startswith("VmHWM:"):
                                vmhwm = max(vmhwm, int(line.split()[1]) * 1024)
                                samples += 1
                                break
                except (FileNotFoundError, ProcessLookupError, PermissionError):
                    pass
                if process.poll() is not None:
                    break
                if time.perf_counter() - started > CHILD_TIMEOUT:
                    process.kill()
                    process.wait()
                    out_file.seek(0)
                    err_file.seek(0)
                    stdout, stderr = out_file.read(), err_file.read()
                    raise BenchmarkError(
                        f"child exceeded {CHILD_TIMEOUT:.0f}s: {command}\n"
                        f"stdout={stdout.decode(errors='replace')}\n"
                        f"stderr={stderr.decode(errors='replace')}"
                    )
                time.sleep(SAMPLE_SECONDS)
            process.wait()
            out_file.seek(0)
            err_file.seek(0)
            stdout, stderr = out_file.read(), err_file.read()
        finally:
            if process.poll() is None:
                process.kill()
            process.wait()
    if not samples or not vmhwm:
        raise BenchmarkError(f"could not sample VmHWM for pid {process.pid}")
    return {
        "exit_code": process.returncode,
        "wall_seconds": time.perf_counter() - started,
        "sampled_vmhwm_mib": vmhwm / MIB,
        "vmhwm_samples": samples,
        "stdout": stdout.decode("utf-8", errors="replace"),
        "stderr": stderr.decode("utf-8", errors="replace"),
    }


def child_env(config_dir: Path, endpoint: str) -> dict[str, str]:
    env = os.environ.copy()
    env.update({
        "BOOXCTL_CONFIG_DIR": str(config_dir),
        "BOOXCTL_CREDENTIAL_STORE": "file",
        "BOOXCTL_TOKEN": "fixture-token",
        "BOOXCTL_TEST_API_BASE_URL": endpoint,
        "BOOXCTL_TEST_OSS_BASE_URL": endpoint,
    })
    for name in ("BOOXCTL_TEST_AUTH_BASE_URL", "DISPLAY", "WAYLAND_DISPLAY"):
        env.pop(name, None)
    return env


def parse_push(stdout: str) -> dict:
    lines = [line for line in stdout.splitlines() if line.strip()]
    if len(lines) != 1:
        raise BenchmarkError(f"expected one JSON line from CLI, got {len(lines)}: {stdout!r}")
    try:
        value = json.loads(lines[0])
    except json.JSONDecodeError as error:
        raise BenchmarkError(f"invalid CLI JSON: {stdout!r}") from error
    if value.get("schema_version") != 1 or value.get("ok") is not True:
        raise BenchmarkError(f"CLI did not report success: {value!r}")
    return value


def verify(child: dict, fixture: FixtureServer, size: int, expected_sha256: str,
           *, cli: bool) -> dict:
    if child["exit_code"] != 0:
        raise BenchmarkError(
            f"child exited {child['exit_code']}\nstdout={child['stdout']}\n"
            f"stderr={child['stderr']}\nfixture errors={fixture.errors!r}"
        )
    stats = fixture.state.snapshot()
    checks = (
        (stats["oss_puts"] == 1, f"expected one OSS PUT: {stats}"),
        (stats["oss_bytes"] == size, f"expected exactly {size} bytes: {stats}"),
        (stats["oss_lengths"] == [size], f"unexpected Content-Length: {stats}"),
        (stats["oss_sha256"] == expected_sha256, f"received SHA-256 {stats['oss_sha256']} != {expected_sha256}"),
        (stats["oss_content_type"] == "application/pdf", f"unexpected Content-Type: {stats}"),
        (not fixture.error_snapshot(), f"fixture errors: {fixture.error_snapshot()!r}"),
        (stats["oss_elapsed_seconds"] > 0, f"missing upload elapsed time: {stats}"),
    )
    for ok, message in checks:
        if not ok:
            raise BenchmarkError(message)

    report = None
    if cli:
        report = parse_push(child["stdout"])
        data = report.get("data", {})
        files = data.get("files", [])
        if data.get("accepted") != 1 or len(files) != 1:
            raise BenchmarkError(f"CLI did not accept exactly one file: {report!r}")
        result = files[0]
        if (result.get("outcome"), result.get("stage"), result.get("size")) != (
            "accepted", "save_and_push", size
        ):
            raise BenchmarkError(f"CLI file result mismatch: {report!r}")
        if stats["message_puts"] != 1 or stats["submissions"] != 1:
            raise BenchmarkError(f"expected one message PUT and submission: {stats}")
        if not stats["message_id"]:
            raise BenchmarkError(f"saveAndPush had no message id: {stats}")
    elif stats["message_puts"] or stats["submissions"]:
        raise BenchmarkError(f"buffered control must only PUT OSS: {stats}")

    return {
        "verified_bytes": stats["oss_bytes"],
        "verified_sha256": stats["oss_sha256"],
        "oss_puts": stats["oss_puts"],
        "oss_content_length": stats["oss_lengths"][0],
        "oss_content_type": stats["oss_content_type"],
        "message_puts": stats["message_puts"],
        "save_and_push_requests": stats["submissions"],
        "fixture_upload_seconds": stats["oss_elapsed_seconds"],
        "fixture_upload_mib_s": size / MIB / stats["oss_elapsed_seconds"],
        "wall_mib_s": size / MIB / child["wall_seconds"],
        "fixture_requests": stats["requests"],
        "cli_report": report["data"] if report else None,
    }


def run_case(kind: str, command: list[str], env: dict[str, str],
             fixture: FixtureServer, size: int, digest: str, run_number: int,
             cli: bool) -> dict:
    child = run_child(command, env)
    return {
        "kind": kind,
        "size_mib": size / MIB,
        "run": run_number,
        "command": command,
        "exit_code": child["exit_code"],
        "wall_seconds": child["wall_seconds"],
        "sampled_vmhwm_mib": child["sampled_vmhwm_mib"],
        "vmhwm_samples": child["vmhwm_samples"],
        "stderr": child["stderr"],
        **verify(child, fixture, size, digest, cli=cli),
    }


def streaming(binary: Path, path: Path, size: int, digest: str,
              run_number: int, config_dir: Path) -> dict:
    with fixture_server() as fixture:
        command = [str(binary), "--json", "--timeout", "300", "push", str(path)]
        return run_case("streaming_cli", command, child_env(config_dir, fixture.endpoint),
                        fixture, size, digest, run_number, True)


def buffered(path: Path, size: int, digest: str, run_number: int) -> dict:
    with fixture_server() as fixture:
        command = [sys.executable, str(Path(__file__).resolve()), "_buffered_client",
                   fixture.endpoint, f"/uid-fixture/push/control-{uuid.uuid4()}.pdf", str(path)]
        return run_case("buffered_python_control", command, os.environ.copy(),
                        fixture, size, digest, run_number, False)


def run_buffered_client(argv: list[str]) -> int:
    endpoint, object_path, file_path = argv
    payload = Path(file_path).read_bytes()
    target = urlsplit(endpoint)
    connection = http.client.HTTPConnection(target.hostname, target.port, timeout=CHILD_TIMEOUT)
    try:
        connection.putrequest("PUT", object_path)
        connection.putheader("Content-Type", "application/pdf")
        connection.putheader("Content-Length", str(len(payload)))
        connection.putheader("Connection", "close")
        connection.endheaders()
        connection.send(payload)
        response = connection.getresponse()
        response.read()
        if not 200 <= response.status < 300:
            raise BenchmarkError(f"buffered control PUT failed: HTTP {response.status}")
    finally:
        connection.close()
    return 0


def environment(binary: Path) -> dict:
    cpu = memory = None
    try:
        for line in Path("/proc/cpuinfo").read_text(encoding="utf-8").splitlines():
            if line.startswith("model name"):
                cpu = line.split(":", 1)[1].strip()
                break
    except OSError:
        pass
    try:
        for line in Path("/proc/meminfo").read_text(encoding="ascii").splitlines():
            if line.startswith("MemTotal:"):
                memory = int(line.split()[1]) * 1024
                break
    except OSError:
        pass
    try:
        commit = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], text=True, stderr=subprocess.DEVNULL, timeout=5
        ).strip()
    except (OSError, subprocess.SubprocessError):
        commit = None
    return {
        "platform": platform.system(),
        "machine": platform.machine(),
        "python": sys.version.split()[0],
        "cpu": cpu,
        "cpu_count": os.cpu_count(),
        "memory_bytes": memory,
        "git_commit": commit,
        "binary": str(binary),
        "binary_bytes": binary.stat().st_size,
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
    }


def main(argv: list[str]) -> int:
    if argv and argv[0] == "_buffered_client":
        return run_buffered_client(argv[1:])
    parser = argparse.ArgumentParser(description="Opt-in Linux loopback CLI upload benchmark.")
    parser.add_argument("--binary", type=Path, default=Path("target/release/booxctl"))
    parser.add_argument("--runs", type=int, default=1, help="runs at each fixed size (default: 1)")
    parser.add_argument("--skip-buffered-control", action="store_true")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    if not sys.platform.startswith("linux"):
        parser.error("sampled VmHWM via /proc requires Linux")
    if args.runs < 1:
        parser.error("--runs must be positive")
    binary = args.binary.resolve()
    if not binary.is_file():
        parser.error(f"CLI binary not found: {binary}; build with: cargo build --locked --release -p booxctl")

    runs = []
    with tempfile.TemporaryDirectory(prefix="booxctl-upload-benchmark-") as tmp:
        root = Path(tmp)
        payloads = {}
        for mib in SIZES_MIB:
            path = root / f"benchmark-{mib}MiB.pdf"
            payloads[mib] = (path, create_payload(path, mib * MIB))
        for mib in SIZES_MIB:
            path, digest = payloads[mib]
            for run in range(1, args.runs + 1):
                runs.append(streaming(binary, path, mib * MIB, digest, run, root / "config"))
        if not args.skip_buffered_control:
            path, digest = payloads[CONTROL_MIB]
            runs.append(buffered(path, CONTROL_MIB * MIB, digest, 1))

    result = {
        "schema_version": 1,
        "generated_at_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "environment": environment(binary),
        "method": {
            "transport": "booxctl --json push through loopback API/OSS fixture",
            "sampled_memory": (
                "child VmHWM from /proc every ~1 ms; fixture excluded; "
                "a peak in the final sampling interval may be missed"
            ),
            "fixture_socket_timeout_seconds": SOCKET_TIMEOUT,
            "verification": "exact bytes/hash/Content-Length, one OSS PUT, one message PUT and saveAndPush",
            "scope": "loopback and synthetic credentials only; no production claims",
        },
        "runs": runs,
    }
    if args.output:
        args.output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    if args.json:
        print(json.dumps(result, indent=2, sort_keys=True))
        return 0
    print("kind                    MiB  wall_s  OSS_MiB/s  sampled_VmHWM_MiB  bytes")
    for run in runs:
        print(f"{run['kind']:<22} {run['size_mib']:>5.0f} {run['wall_seconds']:>7.3f} "
              f"{run['fixture_upload_mib_s']:>10.1f} {run['sampled_vmhwm_mib']:>17.3f} "
              f" {run['verified_bytes']}")
    print("Exact byte count/hash/length, single PUT, and CLI report verified. "
          "Memory is sampled child VmHWM; the final interval may hide a peak.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main(sys.argv[1:]))
    except BenchmarkError as error:
        print(f"benchmark_cli_upload: {error}", file=sys.stderr)
        raise SystemExit(1)
