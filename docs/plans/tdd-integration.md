# CLI 集成验证记录

日期：2026-10-29。用户授权按 `cli-review-and-tdd.md` 实施，S1–S4 为公共测试边界。实施使用三个 DeepSeek V4.1 Flash / high 子 agent；主 agent 负责书库、协调、集成和审查。接手时工作区已有未提交实现；不将已有实现的回归测试冒充 Red/Green。

## 架构与功能

- 新增独立 `sandflos-cli`，二进制 `sandflos`，默认 workspace 成员为 core 与 CLI。
- `auto-launch` 从 core 移到桌面 app；CLI 的依赖闭包单独检查。
- CLI 独立认证、配置与 JSON 输出；推送使用逐文件报告，队列操作不刷新桌面仪表盘。
- Calibre 与 Zotero 使用显式目录和只读 SQLite；WebDAV 有大小限制、路径检查和临时文件清理。
- 三平台 CI 运行 CLI 测试、lint、release 构建与产物 smoke；macOS 另设桌面编译检查。

## 证据与审查

书库 Red/Green 和回归记录见 `cli-libraries-execution.md`。早期认证记录见 `tdd-auth.md`，其中列出的未完成项以本次收尾记录为准。其他模块的实际执行记录随各 agent 完成写入。

审查期间修复的重点：导入 token 前服务端验证、凭据按配置目录隔离、文件原子替换、HTTP 错误脱敏、配置错误退出码、取消处理、clippy 告警、重复测试属性与认证测试夹具。最后一轮发现文档列出的 `diagnostics` 尚未接入，现已补上离线、脱敏的诊断命令及其进程测试；`--verbose` 现在在失败时向 stderr 追加错误分类与退出码。

## 验收边界

本机为 Linux。macOS/Windows CI 尚未在本机执行，平台凭据后端、Windows ACL、终端二维码可扫码性、真实云端推送和设备接收均需单独验收。云端接受不等于设备已收到。配置了 CI 不代表 CI 已通过。

## 最终本地验证

- `cargo fmt -p boox-core -p sandflos-cli -- --check`：通过。
- `cargo test --locked -p boox-core -p sandflos-cli`：80 core unit、8 library integration、1 credential lock、1 push cancellation、2 CLI binary unit、13 auth、1 migration、7 process contract、8 push/queue，共 121 项，全部通过。
- `cargo clippy --locked -p boox-core -p sandflos-cli --all-targets -- -D warnings`：通过。
- `bash scripts/check_cli.sh`：执行 rustfmt、测试、CLI lint、release 构建、依赖边界与无显示 smoke。
- 未提交推送；最终提交与 `cli` 分支推送由协调 agent 在整体审查后统一执行。

## 产物下载验收补充

`scripts/check_cli.sh` 现在还会运行打包脚本单元测试，并在 release 构建后生成 `target/cli-package`、核对 SHA-256、可执行位与无显示启动。`CLI` workflow 新增三平台 `verify-package` 任务，通过 GitHub API 下载本 run 上传的 ZIP 后再在对应平台解压执行。实现和证据见 `cli-artifact-verification.md`。本地已验证 7 项脚本测试、全新打包，以及 CI run `40963914110` 的真实 Linux artifact；macOS/Windows 新任务需下一次 CI 运行确认。
