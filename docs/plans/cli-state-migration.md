# 桌面运行时状态迁移评估

2026-10-31 最新：桌面刷新第一片已完成。新显式token/upload/date/share输入与Result入口保留错误，app失败保留旧snapshot/time并通知、成功更新自身snapshot/tray且不写core cache。端点注入纯设备投影无WS/LAN副作用，生产能力保留。12项公共测试及父完整267Rust/7Python检查通过；GPUI缺aes缓存未完成类型检查。旧credential/upload过渡桥仍在，完整登录/上传会话迁移未完成；下文为历史评估，当前顺序见cli-next-steps-and-debt.md。

2026-10-31 下一步与技术债排序见 `cli-next-steps-and-debt.md`：query、queue取消现已补齐，下一步接续桌面登录持久化、刷新错误、上传互斥与进度迁移。用户允许必要的内部重构；当前仍保留公共兼容入口，不自动删除或改写用户认证缓存。

2026-10-30 续实施更新（工作区，尚未提交）：下文评估时缺少的两条公共接口现已补齐。`auth::LoginPortal::start` 由应用提供令牌交付回调并持有 portal；保存失败可重试、成功只交付一次，shutdown/drop 实际回收线程。`PushProgress::Read` 与 `PushBackend::upload_with_progress` 为真实 PushService/CLI 路径提供有界、单调的本地读取进度，`Uploaded` 仍只表示 OSS 确认。证据见 `tdd-login-delivery.md`、`tdd-read-progress.md`。这些是桌面迁移的前置接口，桌面 app-owned 状态、凭据策略和调用路径尚未迁移；下文的历史评估不应再把接口本身记为缺失。登录上游等待仍可能使 shutdown 等待当前 20 秒请求，应用回调必须有界返回。

日期：2026-10-30。基线：`406ef53720dabf1df412359558dc8b8bc2002940`。范围是本轮 `read_agents_state` 的只读架构评估；除本文件外未修改生产代码、测试或现有文档。环境为 Linux，`rustc 1.97.1`、`cargo 1.97.1`。

## 结论

本轮可以完成一次有价值的 app-owned 状态迁移，但不能在现有公共接口不变的情况下彻底删除 `crates/core/src/state.rs` 的进程全局。

可行且应优先实施的第一步，是让 GPUI 桌面端自己持有登录凭据、仪表盘快照、上传进度和“单任务上传”互斥状态；公共 `AuthService`、`PushService`、`QueueService`、`QueryService` 已经提供显式凭据、结果和进度，足以支撑这条路径。这样能移除桌面刷新、上传选择、重推和删除路径对全局缓存与全局上传状态的依赖，并为旧桌面兼容入口保留明确适配层。

完整迁移还缺两个真实的公共 seam：

1. 浏览器登录页目前只能把 token 写入 core 全局（`crates/core/src/auth.rs:147`、`:186`、`:419`），应用无法通过回调接收一次且仅一次的成功 token。若只把 `RuntimeState` 从 core 搬到 app，登录页仍会绕过 app，属于表面迁移。
2. `PushService::PushProgress` 只报告文件开始、阶段、OSS 确认后的完成字节和最终结果（`crates/core/src/services.rs:238-259`），没有传输中的连续字节回调；现有 v1 产品进度使用旧的 `perform_native_uploads` 全局写入路径。要做到 app 持有连续进度，应先在 S2 增加受约束的字节进度回调或桌面适配器，不能伪造百分比。

因此，本轮建议采用“先收口桌面可观察状态，再补登录交付与进度 seam，最后删除全局”的分阶段方案。若父 agent 只允许一个最小、安全且完整的切片，先做第 1 阶段：app 持有 dashboard/auth/progress/mutex，显式凭据调用公共服务，旧全局仅保留兼容入口。若要求本轮就删除 `OnceLock`，则不可行；需要第 2 阶段的两个公共 seam 才能诚实完成。

## 现状清单

`crates/core/src/state.rs:18-28` 的 `RuntimeState` 包含六类可变状态：

| 字段 | 当前用途 | 活跃桌面调用方 | 迁移判断 |
| --- | --- | --- | --- |
| `upload_in_progress: Mutex<bool>` | 阻止并发上传 | `try_begin_upload_task` / `finish_upload_task` 在 app、push、Calibre、Zotero | 应由 app 的 `UploadSession` 持有；旧路径保留兼容 |
| `auth_state: Mutex<AuthState>` | 内存 token 和更新时间 | `hydrate_auth_state`、`api::fetch_auth_context_and_user`、浏览器登录页 | 可先由 app 持有；登录页交付需新 seam |
| `upload_runtime_state: Mutex<UploadRuntimeState>` | UI 文本、字节、速度、ETA、最近错误 | 旧 native upload、Calibre/Zotero 推送、dashboard 快照 | 应由 app 持有；连续字节来源需新 seam |
| `dashboard_cache` | 5 秒快照缓存和删除后的局部更新 | `dashboard.rs`、重推/删除、app 刷新 | 可移到 app；UI 已有 `AppState.snapshot` |
| `last_tray_anchor` | Tauri 遗留托盘位置 | 仅 `src-tauri` | 不在 GPUI 迁移范围 |
| `login_portal` | 登录回环服务生命周期，当前值只写不读 | `core/auth.rs` | 登录交付 seam 完成后可移出，当前无产品读取方 |

`crates/core/src/state.rs:28-34` 的 `static RUNTIME_STATE: OnceLock<Arc<RuntimeState>>` 只在首次调用时初始化，进程内所有调用者共享同一实例。`app_data_dir()`（`:36-41`）还耦合固定目录和忽略建目录错误，但它不是可变运行时状态；迁移时先把它替换为 app 注入的目录，避免与状态收口混成一次大改。

活跃桌面端是 `crates/app`，其 `AppState`（`crates/app/src/state.rs:106-115`）已经有 `snapshot`、加载标志、UI 标签与通知。它仍调用全局的入口是：

- 启动时 `hydrate_auth_state`（`:119`）。
- 刷新后 `set_dashboard_cache`（`:165`）。
- 上传开始/取消/结束的 `try_begin_upload_task` / `finish_upload_task`（`:213`、`:231`、`:260`）。
- 上传、重推、删除、Calibre、Zotero 的旧方便函数（`:243`、`:274`、`:301`、`:368`、`:426`），它们内部重新读取全局凭据、刷新全局 dashboard，并写上传进度。
- `autostart.rs:40` 使用 `core::state::app_data_dir()` 写启动标记。

旧 Tauri 树 `src-tauri` 自己复制了一份 `RuntimeState` 并存入 Tauri `manage`，不依赖 core 全局；按方案 `cli-review-and-tdd.md:59`，它不参与这条迁移。桌面端没有单独的 `RuntimeState`；当前 app 持有的是 UI 状态，不是完整会话状态。

## 已有公共能力

公共 seam 已经比全局入口更强，不需要为迁移重写协议层：

- `AuthService`（`crates/core/src/auth_service.rs:725-831`）通过显式 `AppPaths`、`CredentialStore`、环境 token 和注入 `AuthClient` 完成导入、验证、注销和 QR 登录。`resolve_token` 返回带来源的凭据；`login_with_qr` 在持久化成功后才返回成功。现有测试验证了不同配置目录互不共享 token（`crates/cli/tests/auth.rs:75-100`）、QR 缺 token/拒绝/断网不落盘（`crates/core/tests/auth_qr_failures.rs:132-166`）、超时、取消和有限轮询（`:169-232`），以及并发凭据锁（`crates/core/tests/credential_lock.rs:4-25`）。
- `HttpPushBackend`（`crates/core/src/services.rs:1081-1123`）接收显式凭据、端点和超时；`PushService::push_files`（`:548-842`）返回逐文件 `PushReport`，区分 accepted/failed/unknown，并在真实 HTTP 边界保留消息 ID 和 rev。
- `QueueService`（`:865-985`）对列表、重推和两步删除返回类型化结果；失败分类和“不重复提交”由 `crates/core/tests/http_failure_paths.rs` 覆盖。
- `QueryService`（`:1033-1057`）把 devices/storage/reading 分开；`fetch_identity_for_token_with_endpoints` 允许只读查询不依赖 storage 统计。
- `PushProgress`（`:238-259`）提供文件开始、阶段、OSS 接受后的完成字节和最终结果。CLI 测试 `upload_progress_is_bounded_and_requires_an_oss_acknowledgment`（`crates/cli/tests/http_output_secrets.rs:6-49`）验证上传事件只在远端确认后出现且字节数有界。

这些能力让 app 可以用“解析一次 token → 构造 backend/service → 收集结果 → 更新自己的状态”替代大部分全局调用。

## 推荐的实现

建议保留 `crates/core/src/state.rs` 作为旧调用方的兼容模块，新增一个只属于桌面的显式状态对象，例如 `crates/app/src/runtime.rs` 的 `RuntimeState`。名字可以复用，语义必须变化：它不提供进程全局访问器，由 `AppState` 或 GPUI 启动流程创建并持有。

第一阶段的建议结构：

```text
AppState {
    runtime: RuntimeState,                 // app-owned, not OnceLock
    snapshot: Option<DashboardSnapshot>,  // keep as UI projection
    ...
}

RuntimeState {
    token: Option<String>,                // or a non-Debug credential handle
    updated_ms: Option<u128>,
    upload: UploadRuntimeState,           // current UI shape
    upload_active: bool,
    dashboard: Option<DashboardSnapshot>,
}
```

`RuntimeState` 的字段应由 app 线程更新，后台任务通过一次性请求值和结果回传，避免把 `Mutex` 直接暴露给 UI。凭据可以在内存中保留以驱动当前会话；持久化仍走显式凭据存储，不把 token 写进 dashboard 或通知文本。

第一阶段的具体替换：

1. `AppState::new` 用 app 注入的 `AppPaths` 和 `AuthService` 解析本地凭据，填充 app 的 token/updated_ms；不再调用 `core::state::hydrate_auth_state()`。`AppPaths` 应使用桌面数据目录下的独立子目录或明确的兼容路径，并在迁移文档与测试中保持旧 `auth_cache.json` 不自动迁移、不删除。`cli-review-and-tdd.md:89` 已经规定历史缓存只允许显式导入。
2. 刷新不再调用 `build_dashboard_snapshot()`。由 app 用显式 token 创建客户端和 backend，调用公共查询能力；快照写入 `AppState.snapshot` 和 app 的 dashboard 缓存。仪表盘聚合需要把 `build_dashboard_snapshot` 改成接收 explicit token/upload projection 的公共函数，或由 app 组合 `QueryService` 结果与 `auth_service` 身份信息；后者的公共 seam 更清晰，但会改变 dashboard 聚合职责，宜单独立项。
3. 上传选择路径先保留现有旧函数以降低行为风险，但把互斥从 core 全局移到 app：`upload_active` 在启动任务时检查并设置，任务结束/取消/错误都由 app 的统一收尾恢复。这样即使旧 native 上传内部暂时仍写 core 进度，桌面也不会再依赖 core 的互斥。
4. 重推/删除不再调用自动刷新全局缓存的 `dashboard_push_resend` / `dashboard_push_delete`。用 `push_resend_with_auth_with_endpoints` 或 `QueueService` 的显式凭据路径执行操作，然后由 app 决定局部移除队列项或重新查询，并把结果写入自己的 snapshot。
5. Calibre/Zotero 推送保留现有文件解析，但把最终上传交给显式凭据的 `PushService`；解析函数可以先作为 core 公共用例返回 `UploadItem`/路径，避免 app 复制 SQLite/WebDAV 逻辑。
6. `autostart` 改为接收 app 注入的数据目录，`diagnostics` 同理；这两个是路径依赖，不应继续通过 `core::state::app_data_dir()` 取值。

第一阶段的建议验收都在 S2，不测试私有函数：

- `app_state_loaded_from_one_config_does_not_read_another`：两个临时目录、真实 `AuthService`/文件存储，验证 app 初始化只使用注入目录；第二个目录保持未授权。
- `app_owned_upload_mutex_rejects_a_second_upload`：用公开的 app 状态更新入口或可测试的 `UploadSession` 公共 API，验证第二次开始返回 busy，第一次结束后可再次开始。
- `app_owned_dashboard_update_after_delete_preserves_other_items`：通过公共队列服务或 app 状态 API 删除一个 ID，验证其他队列项和快照时间被正确更新。
- `app_upload_progress_is_cleared_and_error_is_retained`：通过公共进度适配器输入成功/失败事件，验证状态字段与 UI 投影一致，且不泄露 token。
- `browser_login_delivers_token_to_app_once`：只有在第二阶段加入登录回调 seam 后建立；当前不能测试。
- `streaming_push_reports_bounded_progress_before_upload_ack`：只有在第二阶段加入字节回调后建立；不要把现有 `Uploaded` 事件误称为连续进度。

这些测试属于已有公共能力的回归时，必须如实标为 Green；只有先看到行为失败再修实现的行才记为 Red。不要为迁移补造 Red 历史。

## 第二阶段：补全登录与进度 seam

要删除 core 全局，至少需要两个新的公开边界：

**登录交付。** 让浏览器回环登录服务通过显式回调交付成功令牌，例如把 `start_login_flow` 改成接收 app 提供的 `on_token`/完成器，并由 app 保存到自己的 `AuthService`；回环服务仍可保留给桌面适配器，但不能直接写 core 全局。需要覆盖：成功只交付一次、用户取消、二维码过期、token 缺失、回调失败、进程退出无落盘副作用。现有 `AuthService::login_with_qr` 的终端路径已经具备持久化成功语义，可作为设计参照（`auth_service.rs:832-903`）。

**连续上传进度。** 在 `PushBackend`/`PushService` 的公开边界增加受约束的字节进度事件，或提供桌面专用的 public adapter；事件必须明确“读取/发送字节”与“OSS 已确认”的区别，不能把读完文件当作接受。现有 `PushProgress::Uploaded` 保持“远端确认后才发出”的契约，字节事件另设字段/阶段。验收应包含：上限不超总字节、上传失败不报告完成、取消后停止、单个 PUT、进度回调不改变结果分类。CLI 的 `http_output_secrets` 回归继续证明当前 v1 的确认时序。

登录和进度 seam 完成后，才可把旧 `core::state` 的全局入口逐步改为内部兼容包装或删除；`last_tray_anchor` 与 `login_portal` 的遗留生命周期应由 app/Tauri 适配层自行持有，不能再作为 core 的长期状态。

## 风险与限制

1. **凭据格式和目录不是同一件事。** `AuthService` 的 `auth.json` 带 schema 版本、原子替换和 0600/ACL 限制；旧桌面 `auth_cache.json` 是不同格式。迁移必须显式导入或在 app 中保留兼容读取，不要用一次全局替换悄悄改写用户凭据。`docs/cli.md:18` 已明确 CLI 不自动导入或删除桌面配置。
2. **登录成功与持久化成功必须同时成立。** 现有 QR 服务在存储失败时不返回成功；如果 app 仅接收内存 token，刷新后可能丢失会话。先定义 app 的持久化策略和失败呈现，再迁移登录页。
3. **旧上传路径行为不能靠重写保证。** `perform_native_uploads` 当前把整文件读入内存后才发 PUT（`push.rs:157-238`），与 CLI 的流式 `HttpPushBackend` 不同；直接切到 `PushService` 会改变进度事件、错误文本、配额检查时机和部分失败语义。第一阶段应保留旧路径或建立精确 adapter，迁移测试要覆盖未知结果、单次提交和消息 ID 保留。
4. **仪表盘聚合是隐式网络依赖。** `build_dashboard_snapshot` 先取身份和存储，再对多个统计请求用空值兜底（`dashboard.rs:70-136`）；app 若逐项调用 `QueryService`，必须明确哪些字段允许缺失、认证/网络错误如何显示，避免把失败误画成空队列。
5. **全局互斥并不等于上传协议互斥。** 把 `Mutex<bool>` 搬到 app 只解决单进程 UI 并发；如果将来支持多个窗口或独立进程，仍需要明确的持久锁或服务端幂等策略。当前不能把单 app 的 busy 结果包装成跨进程保证。
6. **进度是 UI 状态，不是性能证据。** `UploadRuntimeState` 的字节/速度/ETA 来自旧 native 上传，迁移不能据此声称 200 MiB 峰值内存或生产吞吐改善；性能结论应由独立的 streaming benchmark 记录。
7. **平台验证缺口。** 本轮只进行了 Linux 公共 seam 测试；桌面 GPUI 构建、Windows/macOS 凭据行为和真实托盘/窗口回归未在本评估中执行。不能把 core 测试通过当作桌面全面通过。
8. **并发锁中毒处理。** 现有 `Mutex` 在中毒时回退默认值或忽略更新；app-owned 状态应在 UI 线程集中更新并明确错误，不能复制“锁失败就静默继续”的模式。

## 本轮验证记录

在基线 `406ef53`、Linux `rustc 1.97.1` 下执行：

```bash
CARGO_NET_OFFLINE=true cargo test --locked -p boox-core --test auth_qr_failures --test credential_lock
CARGO_NET_OFFLINE=true cargo test --locked -p boox-core --test push_cancellation --test http_failure_paths
CARGO_NET_OFFLINE=true cargo test --locked -p sandflos-cli --test auth
CARGO_NET_OFFLINE=true cargo test --locked -p sandflos-cli --test http_output_secrets
```

结果：auth_qr_failures 3、credential_lock 1、http_failure_paths 21、push_cancellation 6、CLI auth 13、http_output_secrets 3，全部通过，0 失败。这些是已有公共 seam 的回归证据，不是本次迁移的 Red/Green；没有新增生产实现，因此没有新的 Red 断言。未运行桌面 `cargo check`/构建，也未访问真实凭据、账号、设备或用户书库。

工作区还包含其他 agent 的未跟踪文件（`scripts/benchmark_cli_upload.py` 等）和根 `AGENTS.md`；本评估未修改它们，也未提交或推送任何内容。父 agent 负责选择是否进入第 1 阶段、是否允许第 2 阶段的公共接口变更，以及最终集成与交班。
