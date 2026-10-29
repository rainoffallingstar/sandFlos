# CLI 改造交班记录

日期：2026-10-29。分支 `cli`。交接时的实现提交为 `b7e52bc feat(cli): implement standalone sandflos command-line interface`，之后是跨平台测试与 lint 修复 `d362e7f`、`6306364`。前置提交 `791b361` 只做桌面 autostart 依赖隔离，`9a3e009` 是审查与 TDD 方案。

本文只描述当前可复现的状态。没有执行过的平台、真实账号和真实云端操作都明确标为未验证。

## 1. 现在处于什么进度

M1 与 M2 的命令入口都已在 `crates/cli` 落地，`boox-core` 拆出可注入的服务层和认证服务；本机 Linux 上构建、测试、lint、依赖边界检查和 release smoke 全部通过，远端 Linux/macOS/Windows 三平台 CI 与 macOS 桌面兼容检查也已通过。尚未完成的是端到端协议验收、平台凭据与二维码的人工 smoke、真实云端与设备验收，具体缺口见第 5 节。

交接时 HEAD 为本文档的提交，工作区干净，且已推送到 `origin/cli`。

已实现并可运行：

- `sandflos --help`、`--version`，不创建配置目录、不联网。
- `auth import --token-stdin`、`auth status [--verify]`、`auth logout`、`auth login --qr`；独立配置目录、环境 token 优先、文件凭据原子替换与 0600/0700 权限、导入前服务端验证。
- `push FILE...`，逐文件阶段与结果、部分成功/未知退出 6、200 MiB 与配额前置校验、流式上传请求体（`Body::sized`）。
- `queue list/resend/delete`，默认 30 条上游窗口、删除必须 `--yes`、两步删除部分完成退出 6。
- `devices list/transfer-url`、`storage status`、`reading stats --date`。
- `calibre configure/status/list/push`，只读 SQLite，按 `data-id` 精确取格式。
- `zotero configure/detect/status/list/webdav configure/push`，显式目录、本地附件优先、缺失时 WebDAV 下载并清理临时文件。
- `config migrate`、`diagnostics`，离线、脱敏输出。
- `--json` 单一信封、稳定错误码与退出码；`--verbose` 仅在失败时向 stderr 追加分类行。

## 2. 本地验证证据

在 `/home/fallingstar10/shire/sandFlos` 执行，全部通过：

```bash
cargo fmt -p boox-core -p sandflos-cli -- --check
cargo test --locked -p boox-core -p sandflos-cli
cargo clippy --locked -p boox-core -p sandflos-cli --all-targets -- -D warnings
cargo build --locked --release -p sandflos-cli
bash scripts/check_cli.sh
```

测试合计 121 项、0 失败：core unit 80、`cli_libraries` 8、`credential_lock` 1、`push_cancellation` 1、CLI 单元 2、auth 13、migration 1、process contract 7、push/queue 8。

`scripts/check_cli.sh` 还验证了 CLI normal 依赖闭包不含 gpui/gpui-component/tauri/rfd/tray-icon/auto-launch，并在清空 `DISPLAY`、`WAYLAND_DISPLAY`、`SANDFLOS_TOKEN` 的空临时配置目录下运行 release 二进制的 `--help`/`--version`，确认不会创建配置目录。

Linux 凭据存储需显式 `SANDFLOS_CREDENTIAL_STORE=file` 或使用 `SANDFLOS_TOKEN`；平台默认在 Linux 返回 `credential_store_unavailable`，这是有意行为，不要改成静默回退明文。

三平台 workflow 写在 `.github/workflows/cli.yml`。远端 `CLI` workflow 在提交 `6306364` 上已运行完成：`cli (ubuntu-latest)`、`cli (macos-latest)`、`cli (windows-latest)` 与 `desktop-compatibility` 全部 success。此前 `cc873fb` 与 `d362e7f` 的 macOS/Windows 失败已定位并修复：书库测试改为比较双方 canonicalize 后的路径；平台 keyring 分支的 `needless_return` 已清理。CI 通过仅覆盖自动测试、lint、release 构建与 smoke，不覆盖真实账号、扫码、云端推送与设备接收。

## 3. 关键实现位置

| 关注点 | 位置 |
| --- | --- |
| 参数解析与全局选项 | `crates/cli/src/args.rs` |
| 错误码到退出码映射 | `crates/cli/src/error.rs` |
| JSON 信封与 stderr 约定 | `crates/cli/src/output.rs` |
| 认证、配置目录、凭据后端选择 | `crates/cli/src/auth.rs`、`crates/core/src/auth_service.rs` |
| 推送报告与阶段语义 | `crates/core/src/services.rs` 的 `PushService`/`PushReport` |
| 队列与只读查询服务 | `crates/core/src/services.rs` 的 `QueueService`/`QueryService` |
| HTTP 边界适配器 | `crates/core/src/services.rs` 末尾 `Http*Backend` |
| 书库配置与 SQLite/WebDAV | `crates/core/src/cli_libraries.rs` |
| 原子替换（含 Windows） | `crates/core/src/persistence.rs` |
| 依赖边界与无显示 smoke | `scripts/check_cli_artifact.py` |
| 产物打包与下载验收 | `scripts/package_cli.py`、`scripts/verify_cli_package.py` |

## 4. 审查中已修正的问题（不要回退）

- `save_and_push` 失败或同步未确认时记为 `unknown`，不再计入成功。
- 队列部分删除、重推提交超时保留结构化结果并退出 6。
- `fetch_auth_context_for_token` 让 CLI 使用显式凭据，不读桌面全局登录态。
- HTTP 错误按状态码分类，输出不含服务端响应正文；认证拒绝不回显 body。
- `auth import` 先验证再落盘；环境 token 生效时拒绝导入和扫码。
- 配置写入使用文件锁与原子替换，Windows 不走“先删除旧文件”路径。
- WebDAV 限制下载/解压各 200 MiB、拒绝跨主机重定向、拒绝 ZIP 越界路径与符号链接，失败清理临时目录。
- 书库以只读方式打开，Calibre/Zotero 路径限制在配置目录内。

## 5. 剩余缺口（按优先级）

### P1 端到端协议测试仍缺

现有服务测试多数注入假后端。只有认证适配器有针对回环 HTTP 的测试（`auth_service.rs` 中 `http_adapter_*`）。以下没有真实 HTTP 契约测试：

- 推送的 STS、OSS PUT、neocloud 写消息、同步确认、`saveAndPush` 请求形状与错误分类。
- 队列 `_changes` 读取、`resend`、两步删除的请求路径/方法/鉴权头与 404 语义。
- 查询接口的设备、存储、阅读统计。

建议先加 `crates/core/tests/fixtures/` 下的脱敏样本与回环服务器，再按 T5、T8、T9 的公共边界补测试。`HttpPushBackend` 的上传地址目前由 `build_oss_host` 生成并以 `https://` 固定，若要回环验证需要新增仅测试可用的端点注入点，且不能放宽生产地址校验。

### P1 进程级 happy path 未覆盖

CLI 进程测试目前覆盖参数错误、认证生命周期、迁移和诊断；推送、队列、设备、存储、阅读、Calibre/Zotero 的真实命令只验证了部分前置校验。至少应补：

- 用本地 fixture 服务完成一次 `push` 并断言 JSON `data.files`、退出码和请求序列。
- `queue list/resend/delete --yes` 的成功、404 幂等、部分完成。
- `devices/storage/reading` 的 JSON 字段与错误退出码。
- `calibre push` / `zotero push` 从书库文件进入公共推送流程。

### P2 跨平台与产物验收

- macOS Keychain 与 Windows Credential Manager 已有编译和测试覆盖，但未有真实凭据写入/读取的 smoke 记录；Windows ACL 未验证。
- 终端二维码可扫码性需人工 smoke；自动化只验证非 TTY 拒绝。
- 产物打包与下载验收已补齐：workflow 使用共用打包脚本，三平台 `verify-package` 任务会下载本 run 的 artifact ZIP，检查内容、校验和、可执行位并在无显示环境运行 `--help`/`--version`。实现与本地证据见 `cli-artifact-verification.md`；三平台新任务仍需下一次 CI run 证实。
- 桌面 `send2boox-desktop-gpui` 已在 macOS CI 做 `cargo check` 并通过；本机未构建，桌面人工回归尚未执行。

### P2 真实服务验收

未访问真实 BOOX 账号、OSS、neocloud、设备或生产队列；未执行真实上传、重推、删除。云端接受与设备实际收到是两个层次，需分别记录账号类别、设备、版本与时间。

### P3 小问题

- `crates/core/src/api.rs` 的 `create_client_with_base_url` 只校验 URL，实际仍创建默认客户端；函数名容易误导。认证路径已通过 `_at` 函数使用注入端点，但该 helper 应删除或改名。
- `push` 的 `--` 终止符只在 `push` 子命令保留，其他命令的文件路径含 `-` 时需要用 `--`，行为需在文档中明确。
- `zotero detect` 在 CLI 固定返回 `zotero_detection_unsupported`，这是设计选择，不要在没有跨平台验证前改成自动探测。

## 6. 下一位 agent 的接手步骤

1. `git fetch && git checkout cli && git status`，确认 HEAD 与远端一致、工作区干净。
2. `bash scripts/check_cli.sh` 复跑基线；若只在 Linux，记录凭据后端是显式 `file`。远端三平台 CI 基线为 `6306364`，可作为比较起点。
3. 先做第 5 节 P1 的回环协议测试，按 Red/Green 记录，不要用假后端测试冒充端到端通过。
4. 推送后在 GitHub Actions 查看 `CLI` workflow 的 `cli` 与 `verify-package` 三平台结果，再决定跨平台项是否关闭。
5. 真实账号验收单独执行，不放入日常单测，不提交 token、Cookie、签名 URL 或响应正文。
6. 更新 `docs/plans/tdd-integration.md` 的证据与本文件的缺口状态；不要把未运行的平台或云端项写成通过。

## 7. 交接时的明确结论

本地 CLI 已可构建并在无显示环境启动，核心工作流与脚本契约已实现，121 项测试与严格 lint 通过；三平台 CI 与 macOS 桌面编译检查也已通过。产物下载校验现已实现并完成本地与真实 Linux artifact 验证，三平台新任务待下一次 CI 确认。它还不是“已完成真实云端验收”的状态；主要风险集中在真实 HTTP 契约、进程级 happy path、平台凭据与二维码的人工 smoke，以及真实设备接收。
