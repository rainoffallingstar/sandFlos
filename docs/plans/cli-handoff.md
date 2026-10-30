# CLI 改造交班记录

## 续实施：上传性能与慢连接取消（2026-10-30）

本轮起点为 `406ef53720dabf1df412359558dc8b8bc2002940`，继续使用 DeepSeek V4.1 Flash（high）子 agent，父 agent 集成审查。最终代码提交与 CI 结果将在本轮完成后填入本节；下方此前的 `db3903b`/run `41014099241` 是历史验收，不能证明本轮改动。

- `cli-upload-performance.md`：新增可复现的 Linux 8/200 MiB 真实 CLI 回环上传基准，核对长度、SHA-256、单次 PUT 和 accepted；测量子进程采样 VmHWM，明确最后采样间隔和本机环境限制。
- `cli-slow-cancellation.md`：慢速 OSS 写背压取消的行为 Red/Green 与修复范围；只有 OSS PUT 使用可取消异步边界，其余阶段继续保留既有确认/unknown 语义。
- `cli-state-migration.md`：桌面全局状态调用链及迁移评估；本轮未迁移桌面状态，登录交付、连续进度和平台人工回归仍需后续处理。

本机最终 `CARGO_NET_OFFLINE=true bash scripts/check_cli.sh` 已通过：**198 项 Rust 测试、7 项 Python 测试，0 失败**，fmt、严格 clippy、release 构建、依赖闭包、无显示启动、打包运行通过。日志 `/tmp/sandflos-cli-next-final-check.log`。新构建的 release 二进制完成 8/200 MiB 和缓冲对照测量；200 MiB 采样 VmHWM 为 6.156 MiB，精确内容与单次请求验收通过。基准的无效 CLI 输出 2 MiB 对照在 2.64 秒内被拒绝，无管道死锁；没有将这些已有/审查补验行为补造为 Red。

规范轴与方案轴审查分别见 `cli-next-standards-review.md`、`cli-next-spec-review.md`，各自包含父 agent 处置。客户端构造两行策略与独立夹具的协议样本重复作为判断项保留，未宣称全部维护债务消失。

## 此前已完成的验收

更新：2026-10-30。仓库 `/home/fallingstar10/shire/sandFlos`，分支 `cli`。本轮从 `4d6a48a139851c7f169ed83bd0e1055031d86587` 继续，按原方案及已确认的 S1–S4 由 DeepSeek V4.1 Flash（high）子 agent 实施，父 agent 串联、集成与审查。

本轮最终代码与测试提交为 **`db3903bfa16f3cb2dc26d37cf97e6dcbebccc123`**，已推送至 `origin/cli`；对应 CLI workflow **run `41014099241`，7/7 成功**。其中产品实现提交为 `73f36f8`，`db3903b` 修正跨平台路径测试。`7c2e4fc` 是中间提交，不能单独当作最终验收。后续收尾提交只更新文档，不改变这份已验证代码。

| 最终远端检查 | 结果 |
| --- | --- |
| Linux、macOS、Windows CLI 测试、严格 lint、release 构建和无显示运行 | 3/3 成功 |
| 三平台版本化 ZIP 的生成、校验与运行 | 在各 CLI job 内通过，六份原始包/版本化归档 artifact 均已上传 |
| 从 GitHub 下载产物 ZIP 后校验并运行原生二进制 | 3/3 成功 |
| macOS 桌面兼容编译 | 1/1 成功；不替代桌面人工回归 |

此前 `49117e6` 的 run `40978370161` 只证明旧版本。首次本轮 run `41013482622` 在 macOS/Windows 的新增多库路径断言失败：实现返回 canonical 路径，测试期望却保留原始临时目录拼写。修正测试后，最终 run 通过；详细记录见 `tdd-boundaries.md`。

## 当前功能与本轮补齐

M1/M2 的命令已实现：认证、独立配置、JSON/退出码、本地推送、队列、设备/空间/阅读统计、Calibre、Zotero/WebDAV、迁移与诊断。完整命令见 `docs/cli.md`；逐项完成状态与未验收项见 `cli-plan-status.md`。

本轮重点：

- HTTP 改为类型化错误，保留状态/业务码；错误不带原始响应体或 URL。401/403、429/5xx、业务错误、损坏信封与响应体截断保持正确类别。成功信封必须存在 data；队列必须有 results 数组，不能用空队列吞掉无效响应。
- 重推准备阶段保留认证错误；消息写入、同步和提交不明保留 unknown 与已知身份；非幂等写入只提交一次。同步 revision 不再进入错误文本。
- 过期 STS 在建立上传连接前拒绝；实际 OSS PUT 内容/长度与确认丢失都有验收。上传进度只在 OSS 确认后发出一次，不把读完文件当作远端接受。
- SIGINT 使用可取消 reader；预检、阶段和批次检查同一取消信号。消息响应已到时保留 id/rev；最终提交已确认则保留 accepted，未确认才 unknown；后续文件不启动。真实子进程测试覆盖 8 MiB 慢上传和 saveAndPush 响应丢失，退出 130。
- 凭据锁冲突、注销失败、Windows 实际替换失败、跨年日期/不同时区/null 统计、QR 缺 token/拒绝/超时/取消/断网均有边界验收。Windows 专属替换用例由 Windows CI 执行，Linux 不将它记作已跑。
- Calibre 多库同 ID、全局分页/搜索、完整必需表列校验、500 ms 锁等待、损坏数据库与源字节不变；Zotero 本地/WebDAV、404、凭据不可读、ZIP 损坏/展开超限/符号链接与清理；迁移失败修正后由新进程验证凭据仍可用。
- 固定 Rust 1.97.1；新增独立 cli-v* 发布 workflow、版本化 ZIP 与外部 SHA-256。普通 CLI 三平台 CI 也构建并运行版本化 ZIP，无需创建真实标签。

此前依赖隔离、显式回环端点、权限受限文件凭据、原子替换、队列两步删除、WebDAV 路径/大小限制和临时文件清理继续保留。

## 最新本机验收

Linux 执行：

```bash
CARGO_NET_OFFLINE=true bash scripts/check_cli.sh
```

结果：**196 项 Rust 测试、7 项 Python 进程测试，0 失败**；fmt、严格 clippy、release 构建、CLI normal 依赖闭包、无显示启动、打包与原生二进制运行均通过。随后用本次新构建的二进制生成版本化原生 ZIP，并通过同一消费端校验器。临时日志 `/tmp/sandflos-cli-final-check.log` 不属于交付产物。

统一检查首轮的 Rust/Python 测试已全绿，但 clippy 指出新 SIGINT fixture 的嵌套 if；合并等价条件后完整重跑通过。此 lint 整理不是 TDD Red。

测试使用合成凭据和回环 HTTP、真实 CLI 子进程、临时真实 SQLite；没有访问真实账号、设备、用户书库或平台 keyring。真实 SIGINT 用例限 Unix，Linux 另观察子进程读取计数；Windows 由专属文件句柄替换测试补其平台边界。

## 审查与证据

- `cli-review-followup.md`：规范轴与方案轴分别报告、逐项处置；包含队列空结果误判、Calibre 表结构碰撞、成功确认被取消覆盖和预检取消优先级的修复。
- `tdd-http-errors.md`：类型化 HTTP、信封、队列结构、重推分类、STS 到期的实际 Red/Green。
- `tdd-transfer.md`：取消后的已知消息 ID、真实上传/提交 SIGINT、预检；记录 64 MiB 慢链路实验的限制。
- `tdd-boundaries.md`：书库、WebDAV 与迁移；父 agent 对 Calibre 必需列的追加修复。
- `tdd-calendar-storage.md`：日期、凭据持久化失败与 QR 失败路径。
- `tdd-output-secrets.md`：同步错误脱敏、上传确认丢失与公共进度契约。
- `tdd-release.md`：独立 CLI 发布、工具链和归档验收。

已有实现新增测试首次直接 Green 时明确记录为回归；不补造 Red 历史，也不将编译/下载错误计为行为 Red。早期证据继续见 `tdd-http.md`、`cli-libraries-execution.md`、`cli-artifact-verification.md`；旧数量不替代本节最终数量。

## 尚未完成的验收与架构债务

1. macOS Keychain、Windows Credential Manager 的真实写读、Windows ACL、终端二维码手机扫码。
2. 生产 BOOX/OSS/neocloud、真实 WebDAV、上游业务码及设备实际接收。CLI 的 accepted 表示云端接受，不能声称设备已收到。
3. 200 MiB 峰值内存/吞吐与生产慢链路。64 MiB 慢回环实验曾暴露 socket 写背压；v1 取消依赖 reader、阶段检查与有限请求超时，不能保证瞬间打断已阻塞的系统调用。8 MiB 测试不是大文件性能结论。
4. 桌面人工回归；macOS CI 只执行桌面 cargo check。
5. 真实 cli-v* tag 事件到 draft release 的完整链路。本轮不创建 tag、不发布 release；普通 CI 验证三平台归档脚本与原生产物。
6. 上游统计时区尚未确认，明确输出 timezone:null。请求日期不因本机时区转换，缺失统计保留 null。
7. CLI 已独立于桌面状态，桌面兼容入口仍保留 core 全局状态；原设计“最终由 app 持有状态”的全面迁移尚未完成。

上传进度 v1 为阶段/OSS 确认事件，没有连续百分比；zotero detect 明确不支持，使用显式目录；--timeout 是单次请求超时，读操作轮询可能有多次请求。无可用 keyring 的服务器用 SANDFLOS_TOKEN 或显式 SANDFLOS_CREDENTIAL_STORE=file，不静默明文降级。

## 下一位 agent

1. 先检查 git status/log，保留工作区；以本文件最终代码 SHA 和 CI run 为准，不重复引用旧 run。
2. 读 cli-plan-status.md 和相关切片记录，区分已完成自动化与上述人工/架构事项。
3. 新修复继续走已确认 S1–S4 的真实 Red/Green；已有行为回归直接 Green 诚实记录。
4. 不使用用户真实账号或书库作为日常 fixture，不把实际发布标签当测试资源。若改代码，运行适当检查并等待新提交对应 CI。
