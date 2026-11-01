"""Verify the CLI dependency boundary and run an isolated packaged-binary smoke."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parent.parent
metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--locked", "--format-version", "1", "--no-deps"], cwd=root
))
result = subprocess.check_output(
    ["cargo", "tree", "--locked", "-p", "booxctl", "-e", "normal", "--prefix", "none"],
    cwd=root, text=True,
)
forbidden = {"gpui", "gpui-component", "tauri", "rfd", "tray-icon", "auto-launch"}
found = {line.split()[0] for line in result.splitlines() if line.split()} & forbidden
if found:
    raise SystemExit(f"CLI contains desktop dependencies: {sorted(found)}")
binary = Path(metadata["target_directory"]) / "release" / ("booxctl.exe" if os.name == "nt" else "booxctl")
with tempfile.TemporaryDirectory(prefix="booxctl-smoke-") as temp:
    env = dict(os.environ)
    for key in ("DISPLAY", "WAYLAND_DISPLAY", "BOOXCTL_TOKEN", "BOOXCTL_TEST_AUTH_BASE_URL"):
        env.pop(key, None)
    config = Path(temp) / "config"
    env["BOOXCTL_CONFIG_DIR"] = str(config)
    for option in ("--help", "--version"):
        subprocess.run([str(binary), option], env=env, check=True, timeout=10,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if config.exists():
        raise SystemExit("Help/version created configuration state")
print("CLI dependency boundary and release smoke passed")
