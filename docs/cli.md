# sandFlos CLI

CLI 与 GPUI 桌面端共享 Rust 核心，但无需窗口、浏览器、托盘或显示服务。源码构建：

```bash
cargo build --locked --release -p sandflos-cli
./target/release/sandflos --help
```

Windows 二进制名为 `sandflos.exe`。默认工作区成员为 core 和 cli；构建桌面端请显式指定 `-p send2boox-desktop-gpui`。

## 自动化契约

`--json` 的 stdout 为一个 JSON 对象，包含 `schema_version`、`ok`、`data`、`error`。进度与诊断写 stderr。不要解析人类输出。帮助和版本输出保持文本格式。

退出码：0 成功，1 内部错误，2 参数错误，3 认证错误，4 网络/服务端错误，5 本地配置或 I/O 错误，6 部分完成或结果未知，130 取消。部分完成时检查逐文件结果，避免整批盲目重试造成重复推送。云端接受不等于设备已经下载。

`--config-dir` / `SANDFLOS_CONFIG_DIR` 选择独立配置目录。`SANDFLOS_TOKEN` 为进程级输入，不自动保存；不要把真实 token 放在命令行参数中。默认使用 macOS Keychain / Windows Credential Manager；Linux 等没有持久后端的环境会明确报错，此时使用 `SANDFLOS_TOKEN`，或显式设置 `SANDFLOS_CREDENTIAL_STORE=file` 以启用 0600 权限的本地凭据文件。CLI 不自动导入或删除桌面配置。

`auth import` 会先调用用户接口验证 token，只有验证通过才保存。凭据被拒时退出 3 且不写文件；网络失败退出 4，原凭据保持不变。`auth status` 默认只检查本地凭据，加 `--verify` 才会访问网络。

## 开发验证

```bash
bash scripts/check_cli.sh
```

脚本运行核心和 CLI 测试、CLI lint、release 构建、依赖边界与无显示环境 smoke。CI 为 Linux/macOS/Windows 构建独立二进制，并另设 macOS 桌面兼容性检查。CI 的配置不代表远端任务已经通过；真实扫码、云端推送和设备接收需单独验收。

架构与切片设计见 [TDD 方案](plans/cli-review-and-tdd.md)，实施证据见 [集成记录](plans/tdd-integration.md)。

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

`auth login --qr` 需要交互终端；JSON、重定向和 `--no-input` 模式请使用 token。环境中的 `SANDFLOS_TOKEN` 优先于磁盘凭据，在该变量生效时导入和扫码会被拒绝，避免新凭据被遮蔽。`auth logout` 只能清除 CLI 保存的凭据，不能撤销外部环境提供的 token。

Calibre 的 `data-id` 必须与书库路径一起使用；Zotero 的 `attachment-id` 为附件 ID。书库 SQLite 以只读方式访问。WebDAV 要求 HTTPS（回环测试地址除外），下载附件在推送结束后清理；凭据与 CLI 配置目录隔离。队列上游窗口最多 30 条，列表中的限制标记不能解释为全部历史记录。

退出 6 时检查 `data.files` 中每项的结果和已知消息 ID。结果未知可能表示服务端已接受但响应丢失，应先查询队列，再决定是否重推。退出 3 检查认证；退出 4 检查网络；退出 5 检查本地文件与凭据存储。
