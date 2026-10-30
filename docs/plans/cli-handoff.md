# CLI 改造交班记录

更新：2026-10-30。仓库 `/home/fallingstar10/shire/sandFlos`，目标分支 `cli`。本轮从已推送的 `3d79aef` 继续；打包切片为 `a7b7e35`，业务集成为 `87cf039`。最终代码提交 `25aeab2f5cce398409d2b5d51136b470a61cc0a8` 已推送到 `origin/cli`，CI run `40976698508` 全部通过。后续文档提交不改变这份已验收代码。

## 进度

M1/M2 命令入口与独立 CLI 已实现。本轮补齐真实 HTTP 回环协议、真实二进制进程、书库选书推送和下载产物验收，并修复测试暴露的失败语义。尚未进行真实账号、扫码和设备接收验收，不能将合成协议夹具视为生产服务验收。

已有命令：

- 帮助、版本、诊断、显式配置迁移；离线启动不创建配置目录。
- token 导入/状态/验证/退出、交互终端二维码；独立配置目录，环境 token 优先，平台凭据或显式文件存储。
- 本地文件推送及逐文件报告，流式 OSS body，单文件 200 MiB 与配额前置检查。
- 队列列表、重推、显式 `--yes` 删除；设备列表/互传地址、空间、阅读统计。
- Calibre 配置/状态/列表/按 data-id 推送；Zotero 显式目录、列表、本地附件/WebDAV 与统一推送。

## 本轮审查已修复（不要回退）

- 核心端点通过 `ApiEndpoints` 显式传入，不读取测试环境变量。CLI API-only 测试地址默认同源 OSS；孤立 OSS、非法 origin、非 Unicode 设置明确退出 2，不能回退线上。认证地址采用同一限制。
- `saveAndPush` 与重推写入仅提交一次；重推 502 实际 Red 曾记录到 4 次写入，现在返回 unknown / 退出 6。
- 写消息响应失败保留 unknown 与 resource key；同步未确认或最终提交失败保留消息 ID；不能报告成功。
- 上传配额接口失败立即停止；只读查询/队列只解析身份，不依赖额外统计请求。
- 重推父消息 404 后重建保留原格式，EPUB 不再变为 PDF。
- 队列保留两步删除结果、404 幂等和部分完成；凭据存储错误保持退出 5，变更后端接入取消标记。
- neocloud 不再创建固定 20 秒的隐藏客户端；复用 CLI 客户端，使 `--timeout` 对这类请求生效，路径范围 Cookie 仍保留。
- 日期格式/合法性先于凭据解析；无账号的非法日期仍退出 2。
- WebDAV ZIP 只接受数据库指定附件；含空格密码可持久化，旧密码配置兼容。Base64 仅做格式适配，保护仍取决于凭据后端。
- 打包验证严格检查二进制、README、SHA256SUMS 与权限，拒绝多余文件、危险 ZIP、符号链接；`--force` 拒绝覆盖无关文件或输入目录。

此前的凭据原子替换、配置锁、HTTP 输出脱敏、WebDAV 大小/路径限制与临时文件清理保留。

## 验证

最终代码的本机 Linux 统一检查通过：156 项 Rust 测试、5 项 Python 进程测试，0 失败；fmt、严格 clippy、release 构建、桌面依赖隔离、无显示 smoke、打包与运行验证全部通过。

CI run `40976698508` 在提交 `25aeab2` 上完成，7 项任务全部通过：Linux/macOS/Windows 的 `cli` 检查、三平台 `verify-package` 下载压缩包后验证并运行，以及 macOS `desktop-compatibility` 编译。Windows 跳过两项 Unix 专属 Rust 测试；Python 的 POSIX shell 正常夹具由对应平台的原生 release 包验收补充。

```bash
bash scripts/check_cli.sh
```

该脚本包含 Python 进程测试、Rust 测试、fmt、严格 clippy、release、依赖闭包、无显示启动与本地产物验证。HTTP 测试使用真实适配器和回环 TCP；CLI 测试运行真实子进程；SQLite 使用临时真实数据库。

交班基线 `3d79aef` 的 121 项 Rust 测试与三平台 CLI CI、macOS 桌面编译已通过，基线 run 为 `40963914110`。本轮新增下载验收的证据为上述最终 run。

收尾时实际处理过间歇 CI 失败：run `40974612627` 的 Windows 认证、`40975499550` 的 Windows 查询和 `40975963627` 的 macOS 重推测试在请求阶段提前失败。回环夹具的 listener 使用非阻塞模式，接受的连接却未显式切回阻塞模式；最终提交统一连接模式和读写超时，WebDAV 夹具读取完整请求头。测试断言未放宽。这是跨平台夹具修复，不冒充新增业务功能的 TDD Red/Green。失败日志摘要现按片段写入检查注解，避免 GitHub 注解截断。

Linux 默认无可用凭据后端时，应使用 `SANDFLOS_TOKEN` 或显式 `SANDFLOS_CREDENTIAL_STORE=file`；不要静默降级明文存储。

## 关键文件与证据

| 位置 | 内容 |
| --- | --- |
| `docs/cli.md` | 安装、命令、配置与退出码 |
| `crates/core/src/services.rs` | 公共推送/队列/查询服务与 HTTP 适配器 |
| `crates/core/src/api.rs`、`push.rs` | 显式端点、消息协议与提交语义 |
| `crates/cli/src/auth.rs`、`queue.rs`、`query.rs` | 进程配置、错误映射与参数优先级 |
| `crates/core/tests/http_failure_paths.rs` | 真实 HTTP 协议与失败分类 |
| `crates/cli/tests/query_queue_e2e.rs`、`failure_paths.rs`、`query_validation.rs` | 进程成功/失败/输入契约 |
| `crates/cli/tests/library_push_e2e.rs` | 真实 SQLite 选中格式进入 OSS，确认数据库不变 |
| `scripts/check_cli.sh`、`package_cli.py`、`verify_cli_package.py` | 统一检查、打包、下载后执行 |
| `tdd-http.md`、`cli-libraries-execution.md` | 实际 Red/Green 与回归测试的区别 |
| `tdd-integration.md`、`cli-artifact-verification.md` | 集成与产物证据 |

## 剩余验收与已知限制

- macOS Keychain、Windows Credential Manager 的真实写入/读取和 Windows ACL；终端二维码可扫码性。
- 真实 BOOX/OSS/neocloud 账号、上游业务码及设备接收。云端接受与设备收到分别记录，不提交凭据和签名 URL。
- 200 MiB 文件的实际内存/吞吐、慢链路、连接中断与取消 smoke；自动测试目前证明边界拒绝和流式 body，不能替代性能验收。
- 桌面人工回归；macOS CI 仅执行桌面 `cargo check`。
- `create_client_with_base_url` 仍是仅校验 URL 再创建客户端的旧 helper，命名可后续整理；真实认证请求通过 `_at` 接口指定 origin。
- `zotero detect` 明确返回不支持，使用显式目录；不应未经跨平台验证改成自动探测。
- `--timeout` 是请求级超时，包含 neocloud；读操作轮询可能发生多次请求，不表示整条命令的统一截止时间。

## 下一位 agent

1. 先 `git status`、`git log -5 --oneline`，保留工作区改动；不要假设交班时必然干净，不要覆盖已有实现。
2. 读取本文件与 `tdd-http.md`，只复跑需要的统一检查，不重做已完成切片。
3. 已验收代码为 `25aeab2`、run `40976698508`（7/7 通过）；若有后续代码变更，再查看对应新 run，不重复修复已处理的 CI 夹具问题。
4. 真实账号与平台人工 smoke 单独记录。HTTP 夹具为合成数据，不夸大为生产协议已全量验证。
5. 新修复继续在 S1–S4 公共边界做 Red/Green；不要把并行编译错误计入 TDD Red。
