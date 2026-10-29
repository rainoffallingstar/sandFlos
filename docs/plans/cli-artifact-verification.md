# CLI 产物下载验收

日期：2026-10-29。分支 `cli`。本次补齐交班第 5 节 P2 中“CI 已上传 artifact，但未验证下载后的压缩包可解压运行”的缺口。

## 实现

- `scripts/package_cli.py`：CI 与本地共用同一打包逻辑，输出二进制、`README.md`、`SHA256SUMS`；Unix 显式保留 `0755`，并拒绝错误二进制名和覆盖已有目录（需 `--force`）。
- `scripts/verify_cli_package.py`：可验证解压目录或 GitHub Actions artifact ZIP。检查路径穿越、符号链接、缺失/多余文件、SHA-256、Unix 可执行位，并在清空 `DISPLAY`、`WAYLAND_DISPLAY` 和 token 环境变量后运行 `--help`、`--version`，确认不创建配置目录。
- `.github/workflows/cli.yml`：`cli` 任务改用共用打包脚本；新增三平台 `verify-package` 任务，通过 GitHub API 下载本 run 已上传的 artifact ZIP，再在对应平台解压执行。
- `scripts/check_cli.sh`：先跑打包脚本单元测试，再在 release 构建后打包并验证本地产物。
- `scripts/test_verify_cli_package.py`：覆盖畸形/重复校验和、路径穿越与符号链接、缺失/多余/被篡改文件、相对目录运行、正常打包运行，以及拒绝覆盖仓库目录。

## 已执行证据

- `python3 -m unittest discover -s scripts -p 'test_*.py' -v`：7 项通过。
- `python3 scripts/package_cli.py --output <临时目录>` 后运行 `verify_cli_package.py`：通过，包内二进制权限为 `0755`。
- 下载真实 CI 产物 `sandflos-ubuntu-latest`（run `40963914110`，提交 `3d79aef`，artifact `12937845388`）：`SHA256SUMS` 校验通过，下载目录内二进制保持 `0755`，`--help` 与 `--version` 均退出 0，未创建配置目录。
- 同一 run 的 macOS 与 Windows artifact ZIP 均可下载；Windows `SHA256SUMS` 使用 `sandflos.exe` 与 CRLF，验证器在对应平台按平台二进制名检查。
- workflow YAML 可解析；artifact 查询命令已在本地按 run `40963914110` 解析出 Linux artifact ID。
- 未在本机执行三平台 `verify-package` 任务；该证据需等下一次 CI run。未执行真实账号、扫码、云端推送或设备接收验收。
- 归档校验额外拒绝重复条目、反斜杠路径、盘符/ADS 名称和符号链接；artifact 下载文件固定在工作区根目录，避免 Windows 上 `runner.temp` 的盘符路径在 Git Bash 中被误解析。

## 边界

下载验证覆盖产物内容、校验和、可执行性与无显示启动，不代表桌面端或真实服务可用。macOS/Windows 的验证必须在对应 runner 上执行，不能用在 Linux 上运行异平台二进制替代。
