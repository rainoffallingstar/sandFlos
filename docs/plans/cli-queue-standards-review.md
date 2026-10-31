# 队列切片 Standards 审查

范围：静态阅读基线 `3cc6fb7f29167bd722487c5fde0008c29f3f63ae` 后的工作区新增 `cancellable_queue.rs`、`cli/tests/queue_cancel.rs`、`queue.rs` 及 `api.rs`/`push.rs`/`services.rs` 的 queue 切片；未运行测试、未改产品。审查时修正仍在收尾。

规范违反：未发现明确违反。队列业务位于 core，CLI 仅绑定取消标志和呈现；URL、信封/队列解析、cookie 与签名准备复用既有 helper；新请求前检查取消，测试使用回环、合成 token、绝对读取期限，fixture worker 在 Drop 中实际 join，符合 owned lifetime 与 core/UI 分工。

维护判断（非规范违反）：

1. `cancellable_queue.rs:35-56,58-78,80-111,128-174,176-198` 的“检查取消→发送→状态/正文解析”序列重复。属 Duplicated Code 气味；若继续增加队列端点，可提取仅负责传输的小 helper，保留各响应解析差异。
2. `cancellable_queue.rs:150-290` 的 `resend_request` 同时编排 bucket、文档、签名、写入、确认与 404 fallback，且多个内部函数穿过 `(client, endpoints, auth, flag)` 参数组；属 DivergentChange/DataClumps 风险。当前仅一处复杂编排，暂不值得引入新抽象，后续扩展时再拆。
3. `queue.rs:62-66,116-120` 两次构造后立即绑定取消标志，模式一致且短；属轻微重复，不构成违反。

限制：仅静态审查，未验证运行行为、fixture 竞态或完整测试覆盖。

## 父处置

无确认规范违反。三项维护判断留档，现有共享URL/解析/签名/cookie/同步helper已保持协议一致；后续统一传输时可收口重复请求编排，当前不引入通用框架。父在产品writer结束后运行统一检查，具体证据见交班。
