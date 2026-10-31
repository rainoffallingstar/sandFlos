# 桌面刷新 Spec 审查

固定点：`3cc6fb7` 加审查时 working/untracked。未确认实现缺陷。

新 seam 符合本片合同：`crates/core/src/dashboard.rs` 的 `DashboardRefreshInput` 显式接收 token/upload/日期/share 投影；`fetch_dashboard_snapshot_with_endpoints` 缺 share 时在请求前拒绝，纯设备映射不触发生产 WS/LAN，生产发现仅留在 `refresh_dashboard_snapshot`/legacy。`DashboardError` 保留 stage/status/kind，queue 失败不再变空列表，统计与 device 失败保留，HTTP 200 的 null 仍为合法缺失；错误消息不含响应正文。profile/storage/reading/device/share 投影仍保留。

`crates/app/src/state.rs` 已实际调用 Result API：失败保留旧 snapshot 与 `last_sync_time_ms`、不写 core cache，is_loading 回收并通知；成功更新 app snapshot/tray，未写全局 cache。`crates/app/src/views/push.rs` 仅在合法空快照显示“队列为空”。legacy wrapper 保留旧 soft fallback 与生产能力。

限制：`docs/plans/tdd-desktop-refresh.md` 所述提取后 queue503 Red 没有父可复核的原始日志，最终 Green 与测试通过不能单独证明真实 Red；GPUI 仅见静态接线/环境限制说明，未确认 macOS 运行验证；本次未复跑测试。

## 父集成证据（2026-10-31）

独立Spec结论保留。父完整267Rust、7Python及fmt/clippy/release/产物检查通过；之后仅测试cache恢复修订，12项定向回归与clippy/fmt再次通过。日志分别 `/tmp/sandflos-cli-desktop-refresh-final-check.log`、`/tmp/sandflos-desktop-refresh-targeted.log`。GPUI离线check返回101缺aes缓存，未进入app类型检查；当前代码无远端CI。实现部分闭环，TDD历史/平台限制不标绿。
