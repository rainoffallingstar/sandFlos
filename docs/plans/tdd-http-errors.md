# HTTP 错误分类与提交边界补验

日期：2026-10-30；边界为已确认的 S1/S2/S3。以 `4d6a48a` 为本轮工作基线，沿用既有真实 HTTP 回环夹具。

## 实现与证据

API 返回 `ApiError`，独立保留错误类别、HTTP 状态和业务码；reqwest 错误转换不包含 URL 或响应正文。认证、推送、队列通过类型分配退出码；桌面兼容入口最后才转换为字符串。未知业务码保留为业务/协议错误，不凭猜测当作凭据过期。

以下均通过 `core/tests/http_failure_paths.rs` 的公开适配器/用例观察：

1. 成功信封必须存在 `data`，显式 `result_code` 必须为整数。新增用例先 Red：`{}` 被当作成功的 null；校验信封后 Green。仍兼容旧协议中省略成功码但存在 `data` 的响应；非零业务码优先于 `data` 形状解析。
2. 重推准备阶段的桶配置、文档查询或 STS 返回 401 时，必须退出认证类别且不产生 PUT/POST。先 Red：类型丢失变为 Protocol；`ResendError` 在提交前后均保留 `ApiError` 后 Green。进入写入后无法确认结果的故障继续返回 unknown，禁止盲重试。
3. QR 创建/轮询、阅读统计返回 401/403/429/500 且响应体截断时，状态码必须保留。先 Red：401 变成 Network；将 11 处状态检查移到读取响应体之前后 Green。该用例同时检查正文秘密不出现在错误消息中。

命令：`CARGO_NET_OFFLINE=true cargo test --locked -p boox-core --test http_failure_paths`。前两项修复后 18 项通过；第三项单独 Green，最终总数以统一验收记录为准。相关临时日志为 `/tmp/sandflos-envelope-{red,green}.log`、`/tmp/sandflos-resend-type-red.log`、`/tmp/sandflos-http-contract-green.log`、`/tmp/sandflos-status-body-{red,green}.log`；临时日志不随仓库发布。

修订同步版本号泄漏的 Red/Green 另见 `tdd-output-secrets.md`；它也要求服务层对 Protocol 使用固定安全消息，不能仅依赖底层消息自律。

## 双轴审查后的修复

- `queue_list_rejects_valid_json_without_an_array_of_results`：先 Red，`{"rows":[]}` 被返回为成功空队列；改为必须存在 `results` 数组，缺字段/null/非数组均为 Protocol，Green。空 `results:[]` 仍是正常空队列。日志 `/tmp/sandflos-queue-shape-{red,green}.log`。
- `a_confirmed_submission_is_preserved_when_cancel_arrives_with_the_response`：先 Red，真实 HTTP 适配器收到了最终成功响应却改返 Cancelled；只有请求失败且取消时覆盖为 Cancelled，成功确认保留 Ok，Green。同类同步确认也保留成功结果，后续阶段仍检查取消。日志 `/tmp/sandflos-submit-ack-{red,green}.log`。最终结果区分“已收到成功确认”和“请求已发出但响应不明”，不再把两者统称 unknown。
- 审查整理：移除把所有 String 隐式转换为重推协议错误的 blanket 转换，在缺 storageKey、旧内容转换和 content 解析三个具体位置显式返回类型化安全错误；HTTP 身份与状态不经字符串往返。

## T7 上传前 STS 到期

`core/tests/transfer_contract.rs` 通过真实 `HttpPushBackend::upload` 验证过期 STS 在连接建立前被拒绝，返回稳定 `sts_expired` 协议错误，不泄露密钥。先 Red：建立上传连接后超时并返回 Network；上传前解析到期时间并拒绝已到期凭据后 Green（1 项）。格式错误也明确拒绝，不自动循环刷新或重复上传。

命令：`CARGO_NET_OFFLINE=true cargo test --locked -p boox-core --test transfer_contract`；日志 `/tmp/sandflos-sts-expired-{red,green}.log`。真实云端业务码与时区、真实 STS 刷新行为仍需脱敏生产样本验收；回环协议样本不代表已验证全部线上语义。
