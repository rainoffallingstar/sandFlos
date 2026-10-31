# 后上传阶段 SIGINT 取消

日期：2026-10-30。基线 `3cc6fb7f29167bd722487c5fde0008c29f3f63ae`。范围是 `HttpPushBackend` 的 `write_message`、`confirm_sync`、`save_and_push`，新增 `core/src/cancellable_push.rs`，以及 `api.rs` 中供同一请求计划复用的 crate 内辅助函数；另写本文件与公开 CLI 测试 `crates/cli/tests/post_write_cancel.rs`。未修改上传 body、`PushProgress`、`PushService` 或 `core/push.rs` 的 legacy 入口。没有提交、推送、使用真实账号、凭据或服务器。

## 行为 Red

先运行新增的公开子进程测试：

```bash
CARGO_NET_OFFLINE=true cargo test --locked -p sandflos-cli --test post_write_cancel -- --nocapture --test-threads=1
```

三个用例分别让夹具在收到消息 PUT、同步文档 GET、`saveAndPush` POST 后保持响应不返回。子进程固定 `--timeout 300`，SIGINT 后允许 3 秒退出，整个子进程 watchdog 为 8 秒。旧实现实际结果：

```text
test result: FAILED. 0 passed; 3 failed
sandflos did not exit within 8s
```

这不是编译或夹具启动失败。三个夹具都已记录目标请求并进入“保持响应”阶段；失败原因是客户端停在 `reqwest::blocking` 的响应等待，原子取消标志无法打断请求。`confirm_sync` 的 Red 还证明阻塞轮询不会检查标志直到某次请求返回。

## 实现

`cancellable_push.rs` 提供三条仅在 `HttpPushBackend` 内部使用的异步路径：

- 每次调用建立 owned current-thread Tokio runtime 和 async `reqwest::Client`，沿用 `--timeout` 原值；没有缩短用户超时来让测试通过。
- 请求 future 用 25 ms 分片轮询。先完成已就绪响应，再判断取消；取消后还提供最多 100 ms 的有界收尾窗口，让已经到达或正在写入的响应完成解析，因此确认成功仍可保留身份或 accepted。持续无响应的请求在该窗口后丢弃连接。
- 取消检查位于每个新网络请求之前：syncToken、同步文档 GET、消息 PUT、`saveAndPush`，以及同步轮询的每次迭代。因此在 syncToken 尚未返回时收到 SIGINT，即使该旧请求随后完成，也不会开始消息 PUT；已完成并成功返回的消息写入或提交仍可优先于取消被记录，不会因新门控丢失成功结果。
- 取消时退出 `block_on` 并丢弃请求、响应与 client，随后才释放 runtime。没有分离的阻塞 worker，也没有后台重试。
- `write_message` 只发一次消息 PUT；`confirm_sync` 最多八次同步读取，每次重新获取 sync cookie；`save_and_push` 只提交一次。取消后的错误统一为对应阶段的 `cancelled`，服务保留此前已经收到的 `message_id` / `rev`，未观察到的提交记为 unknown。
- 同步确认把一次 fetched 文档的解析放在重试循环内：格式不完整、版本未到和 HTTP/解析失败都会消费当前尝试并继续下一次，与 legacy `wait_for_remote_push_doc_ready_with_endpoints` 一致。不会因首个格式错误文档提前结束八次尝试。
- 请求文档、URL 和 cookie 选择由 `api.rs` 的 `push_message_doc`、`save_and_push_body`、`push_doc_url`、`save_and_push_url`、`neocloud_cookie_header` 构造；消息写入响应由 `parse_push_message_write_response` 在 blocking legacy 与 async 路径共用。响应继续使用原有状态优先后解析 `parse_envelope_with_status` 和 `parse_push_item_doc`，没有另一套信封或业务码分类。错误由现有 `classify` 转为 `ServiceError`，文本不含响应体、URL、cookie 或 token。
- 阻塞的 `api::*_with_endpoints` 与 `core/push.rs` legacy 入口保持原签名和行为；它们仍使用 blocking client。
- 认证、bucket、STS 和 signed URL 等前置阶段未纳入本切片；它们在 `HttpPushBackend` 中仍可能阻塞，SIGINT 只会在这些调用返回后生效。

## Green 与验证

同一公开测试在最终实现后（含审查追加的两个用例）：

```text
running 5 tests
test malformed_confirmation_document_is_retried_before_the_batch_succeeds ... ok
test sigint_during_confirm_sync_polling_keeps_identity_and_stops_promptly ... ok
test sigint_while_message_write_response_is_slow_exits_130_without_retry ... ok
test sigint_while_save_and_push_response_is_slow_keeps_identity_and_does_not_retry ... ok
test sigint_while_sync_token_is_pending_does_not_start_the_message_put ... ok
test result: ok. 5 passed; 0 failed; ... finished in 2.70s
```

每个取消用例都断言：SIGINT 后 3 秒内退出、退出码 130、`push_incomplete`；第一项阶段与 outcome 正确；后一个文件未开始；目标请求各一次；取消后没有下一次同步或提交。同步用例还断言客户端已收到的消息 ID 被保留；消息 PUT 响应被压住时没有客户端已知 ID；提交用例断言 `saveAndPush` 请求体中的 ID 被保留，且没有第二次提交。

追加的公开子进程用例：

- `malformed_confirmation_document_is_retried_before_the_batch_succeeds`：夹具第一次同步 GET 返回不含 `_id` 的文档，第二次返回 `_rev=1-fixture`。实现必须发出第二次 GET 并最终 accepted。审查修正前的实际 Red 是一次 GET 后退出 6、`stage=confirm_sync`、`outcome=unknown`；因为 `?` 在重试循环内提前结束。修正后该用例 Green，并确认一次消息 PUT、一次提交。
- `sigint_while_sync_token_is_pending_does_not_start_the_message_put`：夹具压住消息阶段第一次 syncToken 响应，SIGINT 后 5 ms 才释放它。旧门控只在复合请求外检查，已到达的 token 响应会在收尾窗口内继续发送消息 PUT。实际 Red 报告保留了生成的 `message_id` 与 `rev=1-fixture`，证明 PUT 已开始；修正后没有消息 ID/rev，`PUT /neocloud/` 为 0，退出 130。该用例连续运行 12 次均通过。
- 同步 token 用例的 `message_id`/`rev` 为空是预期：PUT 未开始，客户端从未收到远端身份。只有已完成的消息写入响应才保留 ID；被压住的消息 PUT 响应也不会把服务端生成的 fixture ID 报告给客户端，因为客户端没有收到它。

重复稳定性检查：新的 syncToken 用例 12/12 通过；消息 PUT 慢响应用例 8/8 通过；格式错误确认重试用例 8/8 通过。

同时运行的现有契约：

```text
http_failure_paths: 21 passed
push_cancellation: 6 passed
transfer_contract: 1 passed
transfer_streaming: 3 passed
```

覆盖状态码与业务码、成功信封必须含 data、截断响应、重定向拒绝、一次性提交、已确认提交在取消竞态下保持 accepted，以及原有取消/流式上传语义。最终实现重跑时曾发现 `a_confirmed_submission_is_preserved_when_cancel_arrives_with_the_response` 失败：响应已成功、取消标志也在同一时点置位，但请求 future 被直接丢弃。加入有界收尾窗口后该用例与其余 core 契约全部通过。本轮最终复跑 `http_failure_paths` 21/21、`push_cancellation` 6/6、`post_write_cancel` 5/5；strict clippy（core、CLI、all targets、`-D warnings`）通过。

## 剩余边界

- 认证、bucket、STS、签名 URL 等前置 HTTP 仍是 blocking client，SIGINT 不能瞬间打断其系统等待。
- 取消时请求已发出但响应未收到，客户端无法知道服务端是否已提交；消息写入和提交仍按 unknown 处理，不自动重试。消息 PUT 尚未开始或响应被压住且未收到时，没有可保留的消息 ID/rev；服务端可能生成的 ID 不会凭空出现在客户端报告里。保留 ID 只是本地已经观察到的身份，不是远端最终状态的证明。
- 25 ms 是取消观察粒度，另有最多 100 ms 的响应收尾窗口；这些是本机边界，不是跨平台延迟承诺。syncToken 新用例在 SIGINT 后固定等待 5 ms 再释放响应，以稳定落入收尾窗口；它不是生产延迟保证。夹具是字面回环和合成凭据，未验证真实慢链路、生产 neocloud、TLS 代理或 DNS 阻塞。
- 每次后写请求创建 runtime/client 有少量开销，未测量生产吞吐。同步轮询仍是固定八次和原有退避，没有新增远端协议语义。
- `cargo fmt` 未对共享 `services.rs` 执行；需待所有 writer 完成后由父 agent 统一格式化、审查、集成和推送。
