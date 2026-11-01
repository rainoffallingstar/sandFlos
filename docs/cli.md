# sandFlos CLI

`cli` 分支只维护命令行工具，不包含桌面端接入，也不需要窗口、浏览器、托盘或显示服务。源码构建：

```bash
cargo build --locked --release -p sandflos-cli
./target/release/sandflos --help
```

Windows 二进制名为 `sandflos.exe`。仓库通过 `rust-toolchain.toml` 固定 Rust 1.97.1，并保留 `Cargo.lock`；CI 使用相同编译器。工作区仍保留桌面包用于历史兼容，但本分支只构建、测试和发布 CLI（`-p boox-core -p sandflos-cli`），CI 不再检查桌面目标。

## 自动化契约

`--json` 的 stdout 为一个 JSON 对象，包含 `schema_version`、`ok`、`data`、`error`。进度与诊断写 stderr。不要解析人类输出。帮助和版本输出保持文本格式。

退出码：0 成功，1 内部错误，2 参数错误，3 认证错误，4 网络/服务端错误，5 本地配置或 I/O 错误，6 部分完成或结果未知，130 取消。部分完成时检查逐文件结果，避免整批盲目重试造成重复推送。云端接受不等于设备已经下载。

非 JSON 推送进度中的 `read <bytes>/<total> bytes` 表示从本地文件读取的字节；`uploaded` 表示 OSS 已确认上传。读完文件不能证明上传或最终推送成功。进度按时间采样，快速文件可能只出现一次读取事件。

SIGINT 可中断 push 中正在等待的认证、配额、bucket、STS、签名链接校验、OSS 上传、消息写入、同步确认和最终提交；退出 130 并停止后续文件。已收到的消息 ID 和已确认成功仍保留，可能已经写入但未收到确认的结果为 unknown，不自动重试。devices、storage、reading 查询也可取消，停止后续查询请求并返回 `cancelled`。queue list/resend/delete也支持取消：写入前取消返回130，可能写入但未确认则保留unknown/部分结果并返回6，已确认成功保留。登录上游的其他阻塞请求仍可能等待当前请求超时；`--timeout` 仍是单次请求超时。

`--config-dir` / `SANDFLOS_CONFIG_DIR` 选择独立配置目录。`SANDFLOS_TOKEN` 为进程级输入，不自动保存；不要把真实 token 放在命令行参数中。默认使用 macOS Keychain / Windows Credential Manager；Linux 等没有持久后端的环境会明确报错，此时使用 `SANDFLOS_TOKEN`，或显式设置 `SANDFLOS_CREDENTIAL_STORE=file` 以启用 0600 权限的本地凭据文件。CLI 不读写桌面配置目录。

`auth import` 会先调用用户接口验证 token，只有验证通过才保存。凭据被拒时退出 3 且不写文件；网络失败退出 4，原凭据保持不变。`auth status` 默认只检查本地凭据，加 `--verify` 才会访问网络。

## 开发验证

```bash
bash scripts/check_cli.sh
```

脚本运行核心和 CLI 测试、CLI lint、release 构建、依赖边界与无显示环境 smoke。CI 为 Linux/macOS/Windows 构建独立二进制并复验下载产物；本分支不再有桌面兼容性检查。CI 的配置不代表远端任务已经通过；真实扫码、云端推送和设备接收需单独验收。

## 发布产物校验

CI 在三个平台分别生成 `sandflos` / `sandflos.exe`、`README.md` 和 `SHA256SUMS`，随后由 `verify-package` 任务下载已上传的 artifact，重新检查并运行。下载后可本地复验：

```bash
# 已解压目录
python3 scripts/verify_cli_package.py ./sandflos-linux-latest

# GitHub Actions artifact ZIP
python3 scripts/verify_cli_package.py ./sandflos-linux-latest.zip
```

校验器会拒绝路径穿越、符号链接和重复条目，核对包内文件集合与 SHA-256，检查 Unix 可执行位，并在清空 `DISPLAY`、`WAYLAND_DISPLAY` 和 token 环境变量后运行 `--help`、`--version`，确认不会创建配置目录。它验证产物本身可解包、可执行，不代表真实账号、云端推送或设备接收已经通过。

本地重新打包：

```bash
cargo build --locked --release -p sandflos-cli
python3 scripts/package_cli.py --output cli-package
python3 scripts/verify_cli_package.py cli-package
```

CLI 独立发布使用 `cli-v<版本>` 标签，例如当前 CLI crate 的 `cli-v0.1.0`，与仓库其他发布流程相互独立。`release-cli.yml` 先核对标签与 `crates/cli/Cargo.toml` 版本一致，再在三个平台执行完整检查、生成原生 ZIP 与外部 `.zip.sha256` 校验和、解压运行校验。三个包全部通过后创建 **draft release**，由维护者检查后发布；流程不覆盖已有发布或标签。

维护者也可从 Actions 手动运行 `CLI release`，选择待验证的分支或 ref，填写与该 ref 的 CLI manifest 版本匹配的标签字符串。手动模式校验和打包所选 ref，不切换到同名标签，该标签也不必已存在；仅上传验收产物，不创建发布或标签。真实标签触发到 draft release 的事件链尚未执行。下载 ZIP 后可使用上述校验器；包内 `SHA256SUMS` 覆盖二进制和 README，外部 `.zip.sha256` 覆盖整个下载文件。

架构与切片设计见 [TDD 方案](plans/cli-review-and-tdd.md)，实施证据见 [集成记录](plans/tdd-integration.md)。

Linux 上可选运行真实 CLI 子进程的回环上传基准，测量 200 MiB 上传的采样峰值 RSS 与吞吐，并校验精确字节/内容与单次 OSS PUT。只有这个大型基准需要手动运行；它不在常规 CI 或 `scripts/check_cli.sh` 中：

```bash
cargo build --locked --release -p sandflos-cli
python3 scripts/benchmark_cli_upload.py --runs 1 --json
```

该脚本使用合成凭据和回环 HTTP 夹具；测量仅代表本机回环路径，不代表生产网络、OSS 或设备接收性能。峰值是约 1 ms 采样的子进程 `VmHWM`，最后一次采样间隔内可能存在未观察到的峰值。方法、实测数据与限制见 [上传性能证据](plans/cli-upload-performance.md)。

## 命令示例

```bash
# 首次使用：交互终端扫码
sandflos auth login --qr

# 无交互环境：通过 stdin 输入 token；不要将秘密直接写在命令参数中
sandflos auth import --token-stdin < /path/to/private-token-file
sandflos --json auth status --verify
sandflos --json push ./book.pdf ./document.epub
sandflos --json queue list --limit 10
sandflos queue resend MESSAGE_ID
sandflos queue delete MESSAGE_ID --yes

sandflos devices list
sandflos devices transfer-url DEVICE_ID
sandflos storage status
sandflos reading stats --date 2026-10-29

sandflos calibre configure --library '/path/to/Calibre Library'
sandflos --json calibre list --limit 20 --offset 0
sandflos calibre push --library '/path/to/Calibre Library' --data-id 7

sandflos zotero configure --data-dir '/path/to/Zotero'
sandflos --json zotero list --limit 20
sandflos zotero webdav configure --url https://example.org/dav --username USER --password-stdin
sandflos zotero push --attachment-id 4
sandflos diagnostics
```

`diagnostics` 是离线命令，不读取网络，也不输出 token。JSON 模式下 `data` 包含版本、平台、配置目录、凭据存储类型、环境 token 是否生效，以及 Calibre/Zotero 的已配置摘要；路径中的用户主目录会替换为 `~`。它不需要 TTY，但会报告配置目录是否存在，不会为了诊断而创建目录。

`--verbose` 在命令失败时向 stderr 追加一行稳定错误分类与退出码，stdout 的 JSON 对象不受影响。

`reading stats --date` 的日期原样作为服务端统计参数传递，不随运行机器的时区换日。当前尚未确认上游统计时区，因此 JSON 明确返回 `timezone: null`，文本返回 `timezone=unspecified`；CLI 不将它解释为本地时间或 UTC。上游缺失的统计保持 `null`，不会补成零。

`auth login --qr` 需要交互终端；JSON、重定向和 `--no-input` 模式请使用 token。环境中的 `SANDFLOS_TOKEN` 优先于磁盘凭据，在该变量生效时导入和扫码会被拒绝，避免新凭据被遮蔽。`auth logout` 只能清除 CLI 保存的凭据，不能撤销外部环境提供的 token。

Calibre 的 `data-id` 必须与书库路径一起使用；Zotero 的 `attachment-id` 为附件 ID。书库 SQLite 以只读方式访问。WebDAV 要求 HTTPS（回环测试地址除外），下载附件在推送结束后清理；凭据与 CLI 配置目录隔离。队列上游窗口最多 30 条，列表中的限制标记不能解释为全部历史记录。

退出 6 时检查 `data.files` 中每项的结果和已知消息 ID。结果未知可能表示服务端已接受但响应丢失，应先查询队列，再决定是否重推。退出 3 检查认证；退出 4 检查网络；退出 5 检查本地文件与凭据存储。
