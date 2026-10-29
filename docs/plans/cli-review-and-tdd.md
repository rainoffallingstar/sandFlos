# sandFlos 纯 CLI 改造审查与 TDD 执行方案

日期：2026-10-29。审查基线：`b4e9047a0d1bbbcdfe662baa20133f58bef853f9`。

本次交付是架构审查和可执行的 TDD 方案，提交到 `cli` 分支；尚未实现 CLI。下文命令、接口、测试文件和 CI 都是目标设计，不能当作已有功能运行。默认在同一仓库增加 CLI 产品形态，保留 GPUI 桌面端；纯 CLI 指安装、启动、登录和主要工作流都不要求 GUI、浏览器、显示服务器或托盘。扫码仍可由手机完成。

## 1. 结论与范围

新增 `crates/cli`，包名建议 `sandflos-cli`、二进制名 `sandflos`，复用 `boox-core`。不要从 `src-tauri` 再复制业务代码，也不要把桌面应用改成一个隐藏窗口进程。

首个可用版本（M1）包含：帮助/版本、独立配置目录、token 导入/状态/退出、无浏览器二维码登录、本地文件推送、队列查询/重推/删除、稳定 JSON 和退出码。完整版本（M2）补齐设备、云空间、阅读统计、Calibre、Zotero/WebDAV、诊断与三个操作系统上的验收。自动启动和托盘不是 CLI 功能；定时运行交给用户现有调度器。本期不承诺设备指定推送、断点续传、并行上传、TUI 或常驻守护进程，代码中没有足够的协议依据支持这些承诺。

## 2. 当前实现审查

以下是基于源码的发现，不是生产故障复现。优先级以 CLI 上线影响排序：P1 必须先解决；P2 在对应能力上线前解决。

| 编号 | 级别 | 证据（相对仓库根目录） | CLI 场景的影响与处理 |
| --- | --- | --- | --- |
| R1 | P1 | `crates/core/src/auth.rs:26`，`start_login_flow` 启动线程后返回，登录界面由 HTML 提供 | 如果 CLI 调用后结束进程，登录服务随之结束。新增同步等待完成/取消/超时的认证服务；CLI 默认直接调用二维码创建/轮询 API，在终端展示二维码，不依赖回环 HTML。 |
| R2 | P1 | `crates/core/src/push.rs:511`，`save_and_push` 错误仅放入 warnings，随后 `success += 1`；`:655` 部分失败仍返回 `Ok(DashboardSnapshot)` | 自动化可能把未完成的推送判为成功。返回逐文件结果，区分已上传、推送已接受、失败、结果未知；最终失败或未知不能退出 0。是否需要兼容该 API 的可忽略错误，必须由协议样本证明，不能一概吞错。 |
| R3 | P1 | `crates/core/src/state.rs:33–109`，`OnceLock`、固定数据目录、忽略建目录/写入/删除错误 | 多测试实例互相污染，登录/退出可能看似成功但未持久化。注入路径与状态；持久化返回 Result；原子写、限制凭据文件权限、并发写互斥。 |
| R4 | P1 | `crates/core/src/api.rs:16,20,56,68`，硬编码服务地址、函数内创建客户端、`Result<_, String>`，HTTP 错误直接包含响应体 | 无法稳定离线验证协议，无法可靠区分网络/认证错误，原样输出可能泄露服务端回显的敏感数据。注入端点和客户端；按状态码/业务码分类；输出和诊断统一脱敏。 |
| R5 | P1 | `crates/core/src/dashboard.rs:70–135`，子查询用 Null/空列表兜底；`push.rs:711` 上传后刷新完整仪表盘 | 查询失败可能被解释成空队列；上传后还要等待不相关请求。按命令独立查询，结果保留错误。仪表盘聚合继续由桌面适配层负责。 |
| R6 | P2 | `crates/core/Cargo.toml` 无条件引入 `auto-launch`、keyring 只列 `apple-native`；`zotero.rs:423` 非 macOS 自动检测直接报错 | “纯 Rust 核心”不代表无桌面运行假设。把自动启动移到 app；按目标平台/feature 提供凭据后端，Linux 无会话环境支持显式凭据输入；非 macOS 提供显式 Zotero 路径。凭据实际可用性需各平台运行验证，不能只看编译。 |
| R7 | P2 | `crates/core/src/push.rs:160–238` 先读完整 Vec 并上报读取进度，再发 PUT；单文件上限为 `200 * 1024 * 1024` 字节 | 终端可能在网络发送前显示“100%”；内存随文件大小增长。改用有长度的流式 request body；读取/发送/服务端确认分阶段报告，读完不等于远端接受；以 200 MiB 明确边界。 |
| R8 | P2 | `crates/core/src/push.rs:828` 忽略 `delete_push_doc` 结果，再调用批量删除 | 可能出现部分删除却报成功；明确两步删除的完成/部分完成/未知结果以及 404 幂等语义。必须用远端协议样本确认各操作职责。 |
| R9 | P2 | `.github/workflows/release-macos.yml` 只在 main 推送触发且发布 Tauri DMG/MSI；`scripts/internal_release_check.sh` 仅检查 src-tauri | cli 分支不会自动获得独立 CLI 验证或产物。新增专用 workflow 与脚本；不要依赖现有桌面发布任务。 |
| R10 | P2 | `crates/core/src/models.rs:5` AuthState 可序列化且带 token；`zotero.rs:384` 除 NoEntry 以外的凭据读取错误也视为已保存 | 不能直接把内部结构当 CLI JSON；状态应区分可用/不存在/不可访问，所有对外 DTO 排除秘密字段。 |

可复用资产：已有 reqwest blocking API、OSS 签名与上传流程、设备地址校验、SQLite 书库查询、二维码 API、领域数据结构。源码有 39 个 `#[test]` 声明，含 Calibre 临时 SQLite 测试；这不是 39 个测试已通过的结论。未发现 AGENTS.md、CONTEXT.md 或 ADR；README 的明确约定是新业务逻辑放 core，UI 放 app。

## 3. 目标架构与迁移边界

```text
sandflos argv / stdin / stdout / stderr
               │
crates/cli: 参数解析、输出 DTO、退出码、终端二维码、信号
               │
boox-core 公共用例: Auth / Push / Queue / Library / Query
               │
外部边界: HTTP、凭据存储、配置文件、SQLite、时间/等待

crates/app → 同一组公共用例 → 桌面仪表盘和进度展示
```

建议 CLI 保持同步执行，复用现有 blocking 网络栈；不为 CLI 引入第二套异步运行时。参数解析候选为 clap derive，版本在实施时根据工具链与依赖解析确定并锁入 Cargo.lock。

按实际测试切片逐步引入以下接口，不先铺设通用框架：

- `AppContext`：配置路径、会话、HTTP 客户端与端点、凭据存储、截止时间/取消信号。每次命令创建实例，不依赖进程全局单例。
- `AuthService`：导入/校验 token、认证状态、注销、二维码创建及等待；成功必须包含凭据可用与保存成功。
- `PushService::push_files`：返回 `PushReport`；每个输入保留路径、阶段、消息 ID（如已产生）、结果码和可恢复提示。进度事件通过回调提供，core 不打印文本。
- `QueueService`：查询/重推/删除；返回操作结果，不刷新仪表盘。列表如果上游只有最近 30 条，必须标记限制，不冒充完整队列或虚构分页。
- 书库服务：路径明确、只读 SQLite、返回类型化条目或待推送文件；复用同一个 PushService。
- `CoreError`：至少区分输入、认证、配置/本地 I/O、网络/协议、取消、部分完成、结果未知。保留脱敏原因和操作阶段，不解析中文错误字符串来分配退出码。

CLI crate 只依赖 core 和 CLI 必需库。`auto-launch` 移到 app；浏览器登录适配器可保留给桌面，但 CLI 的依赖闭包不得包含 GPUI、Tauri、rfd、tray-icon 或 auto-launch。core 中的全局入口暂留给 app 兼容，逐切片切换到新公共用例，最终由 app 持有状态。遗留 src-tauri 不参与这条迁移。

工作区增加 cli member，并建议将 `default-members` 设置为 core 与 cli，使根目录默认构建支持无显示环境；桌面构建继续显式 `-p send2boox-desktop-gpui`。`--workspace` 仍包含 app，不能把它当 Linux 纯 CLI 的验收命令。

## 4. 命令与脚本契约（拟定 v1）

全局参数：`--config-dir PATH`、`--json`、`--no-input`、`--timeout SECONDS`、`--verbose`。CLI 参数优先于环境变量，环境变量优先于持久化配置，最后是默认值。配置目录环境变量为 `SANDFLOS_CONFIG_DIR`，默认使用平台数据目录下独立的 `sandflos-cli` 目录。

| 命令 | 行为和重要限制 | 阶段 |
| --- | --- | --- |
| `sandflos --help` / `--version` | 不读凭据、不建配置目录、不联网；退出 0 | M1 |
| `sandflos auth import --token-stdin` | 从 stdin 读取并校验；禁止 token 参数；默认持久化到凭据后端 | M1 |
| `sandflos auth status [--verify]` | 默认只报告本地配置状态；verify 才请求服务端，状态不输出 token | M1 |
| `sandflos auth login --qr` | 默认登录模式；终端二维码在 stderr，等待成功/拒绝/过期/超时/取消 | M1 |
| `sandflos auth logout` | 幂等清理 CLI 凭据，删除失败非零；说明不能撤销外部环境提供的 token | M1 |
| `sandflos push FILE...` | 等待全部文件得出结果；无文件报参数错误，不弹文件选择器 | M1 |
| `sandflos queue list [--limit N]` | 返回上游支持的范围及 truncated/limit 元信息 | M1 |
| `sandflos queue resend ID` | 等待远端接受或报告失败/未知 | M1 |
| `sandflos queue delete ID --yes` | 只有显式 yes 才删除；缺失时立即拒绝，不挂起等输入 | M1 |
| `sandflos devices list` / `devices transfer-url ID` | 默认打印合法局域网地址，不打开浏览器；找不到/不合法应报错 | M2 |
| `sandflos storage status` / `reading stats --date YYYY-MM-DD` | 独立查询；阅读日期与时区明确，缺失数据不捏造为 0 | M2 |
| `sandflos calibre configure --library PATH` | 支持重复 library；保存并验证；多库以路径+data_id 定位格式 | M2 |
| `sandflos calibre status` / `calibre list --limit N --offset N --search TEXT` | 只读列出书籍及格式 ID | M2 |
| `sandflos calibre push --library PATH --data-id ID` | 精确选择格式后进入公共推送流程 | M2 |
| `sandflos zotero configure --data-dir PATH` / `zotero detect` | 显式路径跨平台；detect 不支持时返回清晰错误 | M2 |
| `sandflos zotero status` / `zotero list --limit N --offset N --search TEXT` | 只读查询，输出 attachment ID、附件可用状态 | M2 |
| `sandflos zotero webdav configure --url URL --username USER --password-stdin` | 密码不进入 argv、JSON 或日志；凭据后端失败必须报告 | M2 |
| `sandflos zotero push --attachment-id ID` | 本地缺失时按配置从 WebDAV 获取，再进入公共推送流程 | M2 |
| `sandflos config migrate --from PATH` / `diagnostics` | 显式迁移旧配置且不改源文件；诊断输出脱敏 | M2 |

`SANDFLOS_TOKEN` 作为 CI 的进程级输入优先于已存凭据，绝不自动落盘。`auth import`/`auth login` 在该变量生效时拒绝执行并解释优先级，避免新凭据被旧环境值遮蔽。默认凭据存储需要平台适配；无 keyring 的服务器可直接用环境 token，或显式选择权限受限的文件存储。不得静默降级为明文文件。Unix 文件权限 0600、目录 0700；Windows 按用户 ACL 限制并单独验收。历史 auth_cache.json 只允许显式导入，成功后仍保留源文件供用户决定。

输出约定：

- 默认 stdout 是人类可读结果；stderr 是进度/诊断。重定向或 `--json` 时禁止 ANSI 动画。
- `--json` 的 stdout 恰好一个完整 JSON 对象并以换行结束；成功和业务错误都采用 `{schema_version:1, ok, data, error}` 信封，error 含稳定 code 和脱敏 message。部分失败时 data 保留全部逐文件结果。认证结果中无 token、密码、Cookie、STS 密钥或带签名 URL。
- 帮助/版本是特例：即使同时给 `--json` 仍输出标准文本。参数解析错误在已识别 `--json` 时输出同一错误信封，否则写 stderr；退出码均为 2。
- 二维码的人类可见展示写 stderr；非 TTY、`--json` 或 `--no-input` 下 `auth login --qr` 拒绝，并提示使用 token 输入，避免没有可见交互却一直等待。认证服务本身仍独立于终端实现。
- SIGINT 优先退出 130，并停止后续副作用；若响应丢失/取消发生于提交阶段，报告结果未知及已知消息 ID，不能声称回滚远端。blocking 请求通过有限超时和阶段间取消检查收敛；v1 不承诺瞬间中断系统调用。

| 退出码 | 契约 |
| --- | --- |
| 0 | 请求完整成功；status 可报告明确的本地未登录状态，调用者应读 authorized 字段 |
| 1 | 未分类内部错误 |
| 2 | 参数/输入错误，或缺少必须的非交互确认 |
| 3 | 需要登录、凭据失效、认证被拒绝 |
| 4 | 网络、服务端、协议或超时错误 |
| 5 | 本地文件/配置/凭据存储错误 |
| 6 | 批量部分成功、远端部分完成或提交结果未知 |
| 130 | 用户取消 |

批量结果聚合：存在成功和失败，或任何未知结果，退出 6；全部失败且无未知，按认证 > 网络 > 本地 I/O > 输入 > 内部错误的优先级选择单一退出码，逐项原因仍完整保留。`auth status --verify` 未授权退出 3。

## 5. TDD 测试边界

本方案先记录待确认的公共测试边界，本次不新增测试。实施前需用户确认以下 S1–S4；这是已读取的 tdd 技能的要求："Before writing any test, write down the seams under test and confirm them with the user." 路径：`/home/fallingstar10/.agents/skills/tdd/SKILL.md`。

| 边界 | 测试入口 | 实体/替代依赖 | 可观察结果 |
| --- | --- | --- | --- |
| S1 CLI 进程 | 编译出的 sandflos 子进程、argv/stdin | 独立临时配置目录、受控环境、回环 HTTP | stdout、stderr、退出码、命令后的公共查询结果 |
| S2 core 公共用例 | AuthService/PushService/QueueService/书库服务 | 真配置文件、真临时 SQLite；HTTP 本地服务器；时钟/等待可控 | 返回值、进度事件、后续公开查询 |
| S3 外部协议 | 注入端点的真实 HTTP 适配器、凭据适配器 | 固定 HTTP 协议样本、测试凭据仓库；平台凭据单独 smoke | 请求协议是否正确、返回值分类、持久化跨进程可见 |
| S4 安装产物 | 各平台 release 二进制 | 无显示服务和空临时目录；假服务完成端到端操作 | 可启动、依赖闭包、无 GUI 副作用、包内容/校验和 |

不 mock 自己的 Service，不测试私有函数，不把日志/数据库内部字段作为业务断言。允许在 S3 检查 HTTP 方法、路径、认证头等对外协议；实际 secrets 用假值。fixture 的预期值来自独立手算或脱敏协议样本，不调用被测函数生成期望值。新测试验证公共行为，已有私有 helper 测试先保留为回归保障。

测试隔离：每个进程使用独立 config-dir；仅清理该子进程继承的 SANDFLOS_TOKEN 等相关变量，不修改开发者 HOME；临时 SQLite 由 fixture 建库、服务只读；CI 不访问真实 BOOX/OSS/WebDAV、不调用真实浏览器/托盘或用户 keyring。动态端口，所有等待有上限；HTTP 重试与二维码轮询通过可控时钟/等待验证，不使用长 sleep。远端端点注入仅用于显式测试上下文，不让普通配置不经约束地转发凭据到任意地址。

## 6. 按纵向切片执行

每一行都是独立的“先一个失败测试 → 最小实现 → 同边界回归 → 审查”的循环；该行的其他边界条件继续逐个循环。不得一次写完整测试矩阵再补实现。失败证据必须是预期的行为断言失败，缺工具链/下载失败不算 Red。重构放在切片审查阶段，先保持公共契约与测试绿灯。

| 切片 | 第一个 Red（测试名示例） | Green 的最小改动 | 后续边界/完成条件 |
| --- | --- | --- | --- |
| T0 构建基线 | 先记录现有测试/依赖结果；这是准备工作 | 建隔离运行方式，解决实际编译阻塞 | 不把既有失败归因于新增 CLI；锁定 Rust/依赖 |
| T1 CLI 骨架 | `help_exits_without_creating_config_or_network`（S1） | 新增 crate、main、参数解析 | version、缺子命令、非法参数、Unicode 路径、JSON 参数错误；纯 CLI 可构建 |
| T2 配置隔离 | `two_config_dirs_do_not_share_auth`（S2） | 显式 AppPaths/Context | 优先级、损坏配置、权限拒绝、原子写失败保留旧值、并发写有明确 busy/串行结果 |
| T3 token 生命周期 | `import_then_verify_in_new_process_is_authorized`（S1/S3） | stdin 导入、校验、凭据存储 | 空输入、失效 token、存储失败、退出失败、幂等退出、环境覆盖；秘密不出现在输出 |
| T4 终端登录 | `qr_pending_then_confirmed_persists_session`（S2） | 复用 QR API，等待成功再退出 | 过期/拒绝/成功无 token/超时/取消/网络失败；非 TTY 拒绝；终端 QR 另做人工可扫码 smoke |
| T5 最小推送 | `one_pdf_returns_accepted_message_id`（S2/S3） | 从现有上传流程提取 PushService，返回领域报告 | fixture 验证 OSS 请求和同步协议；不请求 dashboard；确认的是云端接受，不是设备已经下载 |
| T6 批量语义 | `second_file_failure_returns_partial_report_and_exit_6`（S1） | 每文件阶段和结果汇总、稳定退出码 | 全失败、未知结果、save_and_push 失败不报成功、缺文件/目录/不可读、0 字节、200 MiB 边界、配额不足 |
| T7 进度取消 | `body_read_complete_is_not_reported_as_remote_success`（S2/S3） | 流式 body、阶段事件、有限取消 | 慢服务、连接中断、重复读不超总量、STS 到期、取消释放本地占用；大文件内存验证作为独立性能验收 |
| T8 队列 | `delete_without_yes_performs_no_remote_mutation`（S1/S3） | list/resend/delete | 空队列 vs 查询失败、默认 30 条限制、非法 ID、404、两步删除部分失败、提交超时未知；非幂等操作不盲重试 |
| T9 只读查询 | `devices_list_does_not_fetch_reading_metrics`（S3） | devices/storage/reading 独立用例 | 无设备、跨日日期、接口不可用、合法/非法互传地址、不自动打开 URL |
| T10 Calibre | `selected_library_and_format_push_the_expected_book`（S2） | 显式路径、只读 SQLite、统一推送 | 两库同 ID、缺格式、缺文件、中文标题、分页/搜索、损坏/锁定数据库；不修改源库 |
| T11 Zotero | `missing_local_attachment_downloads_then_pushes`（S2/S3） | 显式配置、WebDAV 凭据和下载、统一推送 | 本地存在不下载、401/404、凭据不可访问、ZIP 路径越界/异常大小/损坏、失败清临时文件、非 macOS 显式路径 |
| T12 迁移诊断 | `migration_keeps_source_and_produces_usable_target`（S1） | 显式旧配置导入、脱敏 diagnostics | 目标已存在拒绝覆盖、损坏源文件、失败可重试；日志与 JSON 不泄密 |
| T13 发布 | `packaged_binary_runs_without_display`（S4） | 三平台 workflow、打包、校验和、CLI 文档 | 真实无显示 Linux、平台凭据 smoke、CLI 端到端、桌面回归、产物可解压运行 |

T0→T1→T2→T3→T4→T5→T6→T7→T8 是 M1 主线；T9–T12 在主线稳定后完成，T13 的基础构建 CI 从 T1 起就建立，每个阶段逐渐增加门禁。不能因只读命令较容易而绕过认证、结果语义和配置隔离。

## 7. 样本、目录与验证方式

建议新增目录（实施时按切片建立）：

```text
crates/cli/src/{main,args,output,error}.rs
crates/cli/tests/{process_contract,auth,push,queue,libraries}.rs
crates/core/tests/{auth_service,push_service,queue_service,libraries}.rs
crates/core/tests/fixtures/{boox,oss,webdav}/
scripts/check_cli.sh
.github/workflows/cli.yml
```

fixture 应覆盖：QR pending/confirmed/expired、token 缺失、401/403/429/5xx、业务码错误、畸形 JSON、STS 过期、上传拒绝、同步未可见、保存推送失败、两步删除部分完成、WebDAV ZIP 正常与越界。不要伪造目前未确认的线上字段语义；有疑问时把脱敏样本与协议假设记入 fixture 说明。真实云端验收单独手动执行并记录账号/设备类别、版本和时间，不进入日常单测。

目标自动化命令（CLI crate 建立后才可执行）：

```bash
cargo fmt -p boox-core -p sandflos-cli -- --check
cargo test --locked -p boox-core -p sandflos-cli
cargo clippy --locked -p boox-core -p sandflos-cli --all-targets -- -D warnings
cargo build --locked --release -p sandflos-cli
cargo tree --locked -p sandflos-cli -e normal
./target/release/sandflos --help
```

若历史 core 格式/lint 不满足门禁，先记录基线并以单独整理提交解决，不把整仓机械改动混入某个业务切片。CI 对 cli 分支 push 及涉及 core/cli/lockfile/workflow 的 PR 触发。Linux/macOS/Windows 分别运行 CLI 测试和构建；Windows 按 exe 路径运行 smoke。macOS 另做 GPUI 构建与现有 core 回归；按实际支持平台维护桌面矩阵，不以 CLI 通过冒充桌面全部通过。

依赖验收使用 CLI 的 normal 依赖图，拒绝 GUI/托盘/auto-launch 包进入依赖闭包；只检查二进制能启动不足以证明独立性。Linux smoke 清除显示环境、使用空目录；打包内容仅二进制、必要说明和校验和，不捆绑桌面资源或用户凭据。CI 可上传构建产物，版本发布由单独 tag 流程触发，不复用 main 上的每日桌面发布标签。

## 8. 完成定义与交付顺序

1. S1–S4 经确认，每个切片保留 Red/Green 命令及结果；实现提交按切片组织，不提前堆砌未来接口。
2. M1 用户能在无显示服务器上导入 token 或通过终端扫码，推送文件并查询/重推/删除队列；全部结果可由 JSON 与退出码判定。
3. M2 的两类书库与 WebDAV、只读查询和迁移全部通过对应 fixture；失败不会修改源书库或泄露凭据。
4. 三平台 CLI 构建/自动测试通过；平台凭据与扫码、真实云端推送/实际设备收到的 smoke 单列证据。云端接受和设备收到是两个验收层次，CLI 默认只保证前者。
5. 桌面仍显式构建，旧配置未被自动迁移或删除；CLI 与桌面需要共享凭据时走明确导入，不隐式竞争全局缓存。
6. README 提供真实可运行的 CLI 安装、认证、推送、JSON/退出码示例；链接本方案；失败恢复说明涵盖结果未知、部分完成和重复推送风险。

建议提交序列：方案 → CLI 骨架/CI → 配置及认证 → 上传结果/流式传输 → 队列 → 查询与书库 → 跨平台发布与说明。每步以测试验证后的可运行行为为单位。

2026-10-29 实施更新：上述 CLI 功能已在 `cli` 分支落地，本地格式、121 项测试、严格 clippy、release 构建和依赖边界 smoke 均通过；三平台 CI、真实云端与设备接收尚未验收。当前证据和剩余边界见 `tdd-integration.md` 与本目录的 `cli-handoff.md`。

## 9. 本次验证记录

- 仓库初始为干净 main，HEAD 如文首；远端查询时不存在 cli 分支。
- 环境：Linux；rustc 1.97.1，cargo 1.97.1。
- 已静态检查 workspace、core/app、遗留 Tauri、发布脚本和 workflow；未运行真实登录/上传/删除，也未访问用户书库或凭据。
- `cargo test -p boox-core --locked`：39 passed、0 failed、0 ignored；doc-tests 0。`cargo metadata --offline --no-deps --format-version 1` 成功。存在既有编译警告；未运行桌面构建或 clippy，不能推断这些检查通过。未实现的 CLI 测试不得记为通过。
- 尝试获取 Cargo 官方文档，但本次网页工具没有返回内容；关于新增依赖的版本和平台后端，实施时仍需核对官方文档与实际解析结果。本审查发现来自仓库源码。
