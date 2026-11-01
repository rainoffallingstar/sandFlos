# 桌面刷新第一片：显式凭据与错误保留 TDD 记录

> **2026-11-01 方向变更（已终止）**：`cli` 分支后续只做纯 CLI，不再接入桌面端（方案 A）。本文档所述桌面切片不再作为本分支待办；未提交的桌面登录会话工作区已撤回并归档在 `/tmp/sandflos-desktop-session-wip`。当前范围与验证见 `tdd-cli-only.md`。以下内容保留为历史记录。

2026-10-31，基线 `3cc6fb7` 加当前 working/untracked 状态。本片只迁移 dashboard 刷新，不声称完成桌面 app-owned 会话或上传迁移。无 commit/push/tag/release，无真实账号、凭据、书库或生产服务请求。

## 公共 seam 与输入

`crates/core/src/dashboard.rs` 新增：

- `DashboardRefreshInput`：显式 token、调用方持有的 `DashboardUploadState`、调用方本地日期、可选 `share_devices` 投影。`Debug` 不输出 token。
- `refresh_dashboard_snapshot`：生产 endpoint 的显式刷新，返回 `Result<DashboardSnapshot, DashboardError>`。调用方提供 `share_devices` 时直接消费该投影；为 `None` 时才走生产 share WebSocket；生产 wrapper 仍执行 LAN 发现。保留现有桌面能力且不重复身份请求。
- `fetch_dashboard_snapshot_with_endpoints`：与上面共用聚合逻辑，但接受 `ApiEndpoints` 和 blocking client，供 literal loopback 公共测试使用。该入口要求显式 `share_devices`，缺失时在发起任何请求前返回 `Input` 错误；它不执行生产 WebSocket/LAN 探测，设备映射走 `device::build_dashboard_devices_pure`。
- `DashboardError { kind, stage, status, result_code, message }`，分类为 Input/Authentication/Network/Business/Protocol。状态 401/403 归身份错误，其它 HTTP 与传输超时归网络；响应 body 不进入 message。
- 旧 `build_dashboard_snapshot` 与 `get_or_fetch_dashboard_snapshot` 保留为兼容适配器，仍读旧全局凭据/上传投影并写旧 cache。只有此兼容路径保留历史 soft fallback：统计/设备请求失败变为 `null`/空设备，queue 失败变为空列表。

严格刷新保留 profile、storage、devices（含合法 `share_devices` 与设备匹配、同局域网互传字段）、queue、阅读统计和调用方上传投影。readingInfo、readTimeInfo、dayRead 与 getDevice 的 HTTP 失败分别以 `reading_info`、`read_time_week`、`day_read_today`、`devices` 阶段返回类型化错误，不再 `unwrap_or(Null)`；HTTP 200 且 `data:null` 仍是合法缺失值。日期由调用方提供，readTimeInfo 与 dayRead 使用同一本地日期。

## Red 证据及可复核性限制

首位实施 agent 报告：先提取保持旧行为的可运行 endpoint adapter，再运行 `queue_failure_is_not_reported_as_a_successful_empty_queue`；回环 queue 返回 HTTP 503 后，结果仍是 authorized 快照和 `push_queue: []`，触发 `expect_err` 失败。其称随后最小修复令该测试通过，属于“提取后行为 Red”，不是未经提取的 baseline 直接 Red。

父 agent 没有取得该次运行的原始日志、完整命令输出或退出状态；现存证据只有实施 agent 报告及初稿叙述。因此这里保留为**已报告但未独立核实的 Red 历史**，不能标作父已验证的 test-first 证据。最终 Green 不能反推 Red，未回滚实现补造历史。公开队列503行为的最终验收与下面的父集成日志可复核。

后续共享机制回归及审查修订为直接 Green：无效 queue JSON、身份401/网络分类与脱敏、完整投影与日期、成功空队列区分、显式凭据/上传及cache隔离、缺share零请求拒绝、各统计/设备失败、HTTP200 null。中间测试路由匹配带query的修订是fixture问题，不计产品Red。GPUI缺依赖不是Red。

## Green 结果

```text
cargo test --locked -p boox-core --test desktop_refresh
running 12 tests
... 12 passed; 0 failed
```

`cargo clippy --locked -p boox-core --all-targets -- -D warnings` 通过。测试夹具使用 literal loopback，请求头与 body 的每次底层读取前都按绝对 deadline 设置读超时，连接写超时和线程 join；`share_devices` 通过显式投影注入，端点到入口不触发 WebSocket 或 LAN 探测。全局状态隔离用例在读取结果后、断言前恢复 auth、upload 与 dashboard cache 原值，避免污染同进程其他用例。

应用 `AppState::refresh_snapshot` 直接在实际 `this.update` 中 match `refresh_dashboard_snapshot` 的结果，没有新增私有 helper 或私有状态测试。成功时由 app 持有 snapshot、更新托盘阅读标签和 `last_sync_time_ms`；失败保留现有 snapshot 与时间，设置可见 `status_notification`。成功和失败路径都在 `this.update` 内先回收 `is_loading`。app 不再写 core dashboard cache。`views/push.rs` 空状态区分三种情况：无 snapshot 且读取中显示“正在读取互动文件队列…”，无 snapshot 且失败显示“尚未取得队列数据，请查看刷新提示后重试”，只有已有合法空快照才显示“队列为空”。app crate 在本机因 GPUI 依赖未缓存、配置 registry 不可达而无法编译执行，不能声称桌面构建或 macOS 人工回归通过；`state.rs` 本片只改刷新 hunk 与 import，未机械重排整个 app 文件。

## 暂存桥与剩余限制

本片仍通过 `get_auth_state().token` 和 `current_upload_snapshot()` 取得凭据与上传投影，这是明确标记的 legacy 暂存桥，不是全局 state 清零。app 的初始化、登录交付、上传互斥/进度、重推/删除仍未迁移；`src-tauri` 旧实现未改。下一片应把登录 token 由 app 持有并通过新 `DashboardRefreshInput` 传入，再迁移上传会话。真实凭据/设备/托盘/窗口回归和三平台 CI 未执行。

## 父集成核对

双轴审查见 cli-desktop-refresh-standards-review.md、cli-desktop-refresh-spec-review.md。父实际运行完整检查日志为 `/tmp/sandflos-cli-desktop-refresh-final-check.log`，返回0：267 Rust、7 Python，fmt/strict clippy/release/依赖边界/无显示/打包运行全部通过。桌面离线编译日志 `/tmp/sandflos-desktop-refresh-app-check.log`：缺少缓存 `aes v0.8.4`，Cargo拒绝离线HTTP下载；属于依赖环境阻塞，没有进入桌面类型检查，不计桌面编译通过。

统一检查后仅修测试cache恢复，产品代码保持。父实际执行 `cargo test --locked -p boox-core --test desktop_refresh`、core all-targets严格clippy及core/CLI fmt，全部返回0：12项公共测试0.20秒通过，日志 `/tmp/sandflos-desktop-refresh-targeted.log`。该维护修订为直接Green。初稿曾经由device映射保留隐式LAN探测路径；最终已抽共享纯映射，不能用最终纯回环保证追认初稿路径。
