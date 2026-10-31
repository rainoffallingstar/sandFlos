
## 前置 HTTP 取消切片 Spec 独立审查（2026-10-31）

范围：最终 `cancellable_prepare.rs`、`services.rs` 准备四方法、`api.rs`/`push.rs` helper、`pre_write_cancel.rs`、`tdd-prewrite-cancel.md`。仅静态审查，未运行测试。

确认缺陷（覆盖）：
1. `pre_write_cancel.rs:432-513` 五例均在 headers 前悬停；`tdd-prewrite-cancel.md` 剩余边界亦承认仅覆盖响应未到达。父审查要求的 API 信封 body stall 公共用例缺失。
2. 无 SIGINT 后立即放行前置响应的竞态用例，不能证明响应完成不会触发下一阶段。

确认缺陷（共享）：`push.rs:312` 已有 `sts_remaining_seconds`，但 `cancellable_prepare.rs:109-112` 复制同一实现；brief 明确要求共享 remaining helper。

生产行为未见其它确认 Spec 缺陷：空凭据请求前拒绝、状态优先、200/206 不消费 body、显式 loopback、三次退避及新请求前取消检查均存在。

源码状态：父审查两项测试修正及 remaining helper 共享仍待处理。

限制：未运行测试，未验证 Windows、连接建立阶段或真实上游。

## 父集成处置

1. `pre_write_cancel.rs:635` 新增 200 响应头后半截 API 信封 body 悬停的真实 CLI SIGINT 用例，退出 130、无后续请求。
2. `:648` 用公开 HttpPushBackend/PushBackend 边界精确设置取消标志，再立即放行成功响应，断言 Cancelled/无 storage。它替换最初依赖 50 ms sleep 的 CLI 竞态用例；属于已确认 S2/S3 的受控竞态验证，其余 6 项真实 CLI SIGINT 证明 S1 信号接线。没有把调度延迟当作取消标志已置位的证据。
3. 新模块直接复用 `push.rs:312` 的 sts_remaining_seconds，签名算法及阈值也保持共享。

最终 7 项通过，日志 `/tmp/prewrite-parent-integration.log`。本轴 3 项发现均已关闭；无新增生产行为缺陷。原审查范围和平台限制仍保留。
