# booxctl

**booxctl** 是 BOOX 的命令行控制器：推送本地书和文档到墨水屏设备，并覆盖登录授权、云空间、设备发现、阅读指标、互动文件队列，以及 Zotero / Calibre 本地书库。

仓库只维护 CLI，不含桌面端代码，也不需要窗口、浏览器、托盘或显示服务。

## 安装与运行

```bash
cargo build --locked --release -p booxctl
./target/release/booxctl --help
```

Windows 二进制名为 `booxctl.exe`。仓库通过 `rust-toolchain.toml` 固定 Rust 1.97.1，并保留 `Cargo.lock`；CI 使用相同编译器。GitHub Actions 为 Linux / macOS / Windows 构建独立二进制与带校验和的发布包。

## 快速开始

```bash
# 登录：默认浏览器打开本地回环登录页，token 回流后保存在系统凭据存储
booxctl auth login

# 查看授权与本地凭据状态（加 --verify 才会访问网络）
booxctl --json auth status

# 上传并推送到设备
booxctl push ./book.epub

# 只读查询
booxctl --json devices
booxctl --json storage
booxctl --json reading
```

## 配置

```bash
export BOOXCTL_CONFIG_DIR="$HOME/.config/booxctl"   # 可选，默认 dirs::data_dir()/booxctl
export BOOXCTL_CREDENTIAL_STORE=file                # Linux 必须；macOS/Windows 默认系统钥匙串
export BOOXCTL_TOKEN=...                            # 可选，进程级，永不落盘
```

Linux 没有可用的系统钥匙串时会明确报错，不会静默降级为明文文件；需要持久化时显式选择 `file`（`auth.json`，0600 权限）。

## 子命令

| 命令 | 用途 |
| --- | --- |
| `auth` | `login --qr` / `import --token-stdin` / `status [--verify]` / `logout` |
| `push` | 上传并推送一个或多个文件 |
| `queue` | `list` / `resend` / `delete --yes` |
| `devices` | `list` / `transfer-url ID` |
| `storage` | `status` |
| `reading` | `stats --date YYYY-MM-DD` |
| `calibre` | `configure` / `status` / `list` / `push` |
| `zotero` | `configure` / `detect` / `status` / `list` / `webdav configure` / `push` |
| `diagnostics` | 离线、脱敏的环境报告 |
| `config` | `migrate --from PATH`（从旧 `auth_cache.json` 显式迁移） |

真实场景配置步骤、凭据后端细节和退出码见 [CLI 使用说明](docs/cli.md)。

## 能力

- **登录授权**：本地回环登录页 + 默认浏览器完成二维码授权，token 直接进入 CLI 凭据存储。
- **直传 OSS**：STS 签名 + 分片上传，`read` 表示本地读取、`uploaded` 表示 OSS 已确认。
- **可中断**：SIGINT 中断认证、配额、bucket、STS、上传、消息写入、同步确认和最终提交，退出 130 并停止后续文件。
- **安全的失败语义**：已确认成功保留；可能已写入但未确认的结果返回 unknown / 部分完成（退出 6），不盲目重试。
- **只读查询与队列**：devices / storage / reading 与 queue list/resend/delete 均可取消。
- **本地书库**：直接读 Zotero `zotero.sqlite`（缺附件时走 WebDAV）与 Calibre `metadata.db`，不复制协议。
- **脚本友好**：`--json` 输出单一 JSON 对象，退出码稳定，stderr 只放进度与诊断。

## 自动化契约

`--json` 的 stdout 为一个 JSON 对象，包含 `schema_version`、`ok`、`data`、`error`。退出码：0 成功、1 内部错误、2 参数错误、3 认证错误、4 网络/服务端错误、5 本地配置或 I/O 错误、6 部分完成或结果未知、130 取消。

详细说明见 [docs/cli.md](docs/cli.md)。

## 开发验证

```bash
bash scripts/check_cli.sh
```

脚本运行核心与 CLI 测试、CLI lint、release 构建、依赖边界检查和无显示环境 smoke。CI 配置通过不代表真实扫码、云端推送或设备接收已经验收。

## 架构

```
booxctl
├── crates/core/          # 纯 Rust 领域逻辑（无 UI 依赖）
│   ├── api.rs            # Send2Boox API / OSS / STS / neocloud
│   ├── auth*.rs          # 回环登录、凭据存储与会话
│   ├── push.rs           # 上传状态机与推送队列
│   ├── cancellable_*.rs  # 可取消的 push / 查询 / 队列阶段
│   ├── device.rs         # 局域网设备识别与互传地址校验
│   ├── zotero.rs         # Zotero SQLite + WebDAV
│   ├── calibre.rs        # Calibre metadata.db
│   └── dashboard.rs      # 仪表盘快照聚合
└── crates/cli/           # booxctl 命令行入口
```

工作区只有这两个 crate；桌面端实现不在本仓库当前分支中。

## License

UNLICENSED · © Fallingstar Team
