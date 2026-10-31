# CLI 本轮 Standards 审查

范围：基线 `3cc6fb7`；已读全部修改及未跟踪生产模块、测试、证据文档。只读，未跑测试、未改生产代码、未提交。

## 硬规则违例

1. `crates/core/src/services.rs:1220-1225` 计算了 `canonical_resource`，却未把换行和 resource 拼入 `string_to_sign`，违反根 `AGENTS.md` 的签名修复要求，也偏离 `crates/core/src/push.rs:202-213`。
2. `crates/core/tests/transfer_streaming.rs:404-406` 的独立 HMAC 期望同样漏掉 canonical resource，测试固化了错误签名；`docs/plans/tdd-read-progress.md:36` 所称测试包含 resource、修复前会失败的证据失真。
3. `docs/plans/tdd-postwrite-cancel.md:55` 仍称被压住的消息 PUT 保留了生成的 `message_id`/`rev`，与 `:56` 及“未收到响应不得声称获得身份”的审查要求冲突。

## 判断项

1. `crates/core/src/cancellable_push.rs:109-132` 重复 `crates/core/src/api.rs:680-701` 的 syncToken 请求和解析，协议规则可能漂移，宜共享。
2. `cancellable_push.rs:143-172`、`:266-295`、`:316-346` 反复传递同一组 `client/endpoints/auth/flag/doc_id/rev`，属于 Data Clumps，宜改为请求上下文。
3. `crates/cli/tests/post_write_cancel.rs:270-323`、`crates/cli/tests/read_progress.rs:264-270`、`crates/core/tests/transfer_streaming.rs:27-209` 各自复制 HTTP 夹具辅助；`cancellable_push.rs:134-141` 的 `parse_json_response` 名称也不揭示其只解码 JSON。

未发现其他硬规则违例。


## 父 agent 处置

最终工作区逐行复核（2026-10-30）：硬规则 1、2 不成立。`services.rs` 的签名串明确包含 `\n{canonical_resource}`，独立测试明确包含 `\n/fixture/fixture/book.pdf`；两者均读取最终磁盘内容确认，不以子 agent 结论替代源码证据。新增真实 HTTP 签名测试和统一检查负责行为验证。

硬规则 3 引用的是修复前 Red：旧代码在取消后非法继续 PUT 并确实收到响应，因此当时报告出现 ID，正是新增门控测试应捕获的错误行为；不能将 Red 描述理解为修复后的契约。另在同文发现一处汇总句仍把消息 PUT 与同步身份合并陈述，已修正为只有客户端收到的身份可保留。

判断项处置：共享信封/Cookie/消息构造与消息写响应解析已落地；async 与 blocking transport 的请求驱动差异保留。参数分组、通用fixture与进度发射器集中化是维护建议，本轮不增加无实际需求的抽象。`parse_json_response` 用于 neocloud 原生文档，调用点上下文明确，与 envelope 解析分工保留。

最终状态：3 项原硬规则发现中，2 项由最终源码反证，1 项是历史 Red 误读；另修正文档汇总句 1 处。判断建议均已评估，未发现未处置的硬规则缺陷。
