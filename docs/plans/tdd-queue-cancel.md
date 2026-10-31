# 队列读取与修改取消：TDD 记录（2026-10-31）

基线 `3cc6fb7f29167bd722487c5fde0008c29f3f63ae`（`cli`）加当前工作区。改动只在本工作区，未提交、未推送；未访问真实账号、凭据、书库或生产 BOOX/OSS/neocloud，未创建 tag/release。所有 fixture 都绑定 IPv4 loopback，使用合成 token、合成队列记录和合成 sync cookie。

## 范围与行为

`HttpQueueReadBackend` 的 `auth_context`、`queue_window`，以及 `HttpQueueMutationBackend` 的 `auth_context`、`resend`、`delete_push_doc`、`delete_message_record` 现在都在自有的 current-thread Tokio runtime 上轮询 async future。CLI 把 `cancel::FLAG` 绑到两个 backend。旧 blocking 公共入口保持兼容。

- `queue list`：身份、syncToken、`_changes` 每一步先检查取消；请求发出后如 SIGINT，future 被 drop，连接随 runtime 关闭。已完成响应如随后观察到取消，不返回列表，避免把取消显示成健康队列。
- `resend`：bucket、文档、STS、签名 URL 与 Range 校验都是提交前步骤；在提交前取消返回 `Cancelled`，不启动 PUT。PUT 已发出但响应未确认时返回 `resend_unknown`，只发一次、不盲重试。已完成的 PUT 成功响应在 runner 的 bounded drain 内保留成功。带 `parent` 的记录在 PUT 成功后还要等待同步 rev；这段确认使用既有 `cancellable_push::confirm_sync_request`，取消映射为 protocol/unknown（退出 6），不会误走输入错误。显式 PUT 404 表示确定的父记录拒绝：清除“已写入”标记后才允许旧 fallback；fallback 的 syncToken 等待中取消仍返回 130，只有 fallback message PUT 真正发出后才可能报 unknown。
- `delete`：先读取文档，再 `DELETE` neocloud 文档，再 `POST /api/1/push/message/batchDelete`。第一步前取消不启动任何写；第一步响应丢失/取消时结果为 `Unknown`，第二步不启动；第二步响应丢失时保留 `remote_delete_attempted=true`、`message_delete_attempted=true`、`complete=false`、`unknown=true`，CLI 仍按既有契约输出 `queue_delete_incomplete` 和退出 6。已确认的第一步或第二步不会被晚到的取消改写成未知。
- 同时存在取消与 unknown 时：真实 SIGINT 令 CLI 退出 130，但结构化错误仍保留 mutation 的 unknown 语义（重推为 `resend_unknown`；删除为 `queue_delete_incomplete` + `unknown:true`）。只读列表在取消时输出 `cancelled`。

状态优先解析、401/403、404 幂等、业务错误和协议错误分类继续走既有 `classify`/`classify_request`。请求错误不含 URL、token、cookie 或响应体。显式 loopback endpoint 注入保持不变。

## 协议复用

没有在 `cancellable_queue.rs` 重新实现协议算法：

- `api.rs` 共享 `queue_changes_url`、`parse_push_queue_response`、`parse_json_document`、`parse_sync_token_payload`、`push_doc_url`、`push_doc_delete_url`、`batch_delete_push_message_url/body`、`parse_existing_push_doc_write_response`。
- `push.rs` 共享 `signed_download_url_with_endpoints`、`is_not_found_push_error`、`update_push_doc_for_resend_at`、`resend_fallback_resource_type`；`ResendError` 增加明确的 `Cancelled` 分支，旧两个变体保持语义。
- `cancellable_prepare.rs` 暴露 async 的 bucket/signed-URL 请求构造；`cancellable_push.rs` 暴露 async sync-token、消息写入和既有 `confirm_sync_request`，由旧同步入口和新队列路径共用解析与轮询算法。队列没有另写同步确认循环。

## 实际 Red

实现前先写一个公共 CLI 用例：fixture 收到 `GET /neocloud/_changes` 后悬停响应，CLI 使用 `--timeout 300`，收到后发送 SIGINT，8 秒 child watchdog：

```bash
CARGO_NET_OFFLINE=true cargo test --locked -p sandflos-cli \
  --test queue_cancel sigint_while_changes_response_is_slow_exits_promptly_without_results \
  -- --nocapture
```

日志 `/tmp/queue-red-changes.log` 的关键失败：

```text
sandflos did not exit within 8s; stdout:  stderr:
test result: FAILED. 0 passed; 1 failed
```

fixture 已记录 `GET /api/1/users/me`、`GET /api/1/users/syncToken` 和 `GET /neocloud/_changes`，不是编译或 fixture setup 失败。实现最小列表取消后同一用例 0.29 秒通过，退出 130，无结果（`/tmp/queue-green-changes.log`）。

## Green 与回归

最终新测试 `crates/cli/tests/queue_cancel.rs` 共 23 项，全部通过（`/tmp/queue-tests-parent3.log`，10.49 秒）：

- 5 项只读/准备 SIGINT：auth、syncToken、`_changes` 响应头悬停、`_changes` body 半截悬停、resend syncToken；均 3 秒内退出 130，且后续请求未开始。
- 4 项 mutation SIGINT：resend PUT 响应头/ body 悬停均 `resend_unknown` 且只一次 PUT；delete 文档 body 悬停不启动 DELETE；delete 第一步响应悬停输出 `queue_delete_incomplete`、`unknown:true`，不启动 batchDelete。
- 2 项 delete 准备 SIGINT：syncToken 与文档响应头悬停，退出 130 且没有 DELETE/batchDelete。
- 1 项第二步响应悬停：保留两步均已尝试的部分完成结构。
- 5 项公开 backend 预取消/受控顺序：预取消 list/resend/delete 都不发请求；取消期间已完成 syncToken 响应不启动文档读取或写出；已完成 list 响应被取消丢弃。
- 2 项 parent 路径：父 PUT 成功后的第二次文档同步悬停被取消，返回 `resend_unknown`；显式 PUT 404 后 fallback syncToken 悬停被取消，返回 130，且没有 fallback message PUT。
- 4 项已确认成功竞态：已完成 resend PUT、delete 第一步、delete 第二步响应在取消晚到时保留已确认结果，不误标 unknown。

这 23 项中只有最初 `_changes` 是实际 test-first Red；其余为同一 runner/协议共享机制的直接 Green 回归，没有补造 Red。中间失败属于实现或夹具错误，不计作产品 Red：在 async 路径里调用同步 bucket/STS/signed-URL/write-message wrapper 会嵌套 runtime；把取消后已确认的删除响应误标 unknown；parent 取消经 input 错误映射会退出 2 而非 unknown 6；以及 parent-404 测试最初数错了 syncToken 序号、把父 PUT 自己的 token 请求而非 fallback 请求悬停。其中嵌套runtime及parent映射由父静态审查发现，修复后由公共回归验证；夹具序号错误属于测试自身问题。最终async resend路径由真实CLI子进程覆盖，不只是编译通过。

既有回归（`/tmp/queue-core-regressions.log`、`/tmp/queue-cli-regressions.log`）：

- `boox-core --test http_failure_paths`：21 passed，覆盖 404 幂等、两步部分完成、401 脱敏、无效 payload、配置 timeout 和 ambiguous resend。
- CLI `failure_paths` 8、`push_queue` 8、`query_queue_e2e` 4 全部通过；queue list/resend/delete 的 loopback URL、cookie、请求体与结果契约未变。

严格 clippy：

```bash
CARGO_NET_OFFLINE=true cargo clippy --locked -p boox-core -p sandflos-cli \
  --all-targets -- -D warnings
```

`/tmp/queue-clippy.log` 通过，无警告。

## 夹具生命周期

每个连接的头和 body 共用一个 5 秒绝对 deadline，逐次 `read` 前设置剩余超时；写超时 5 秒。`finish()` 先置 stop/release，join listener；listener 在 stop 后循环 `accept` 到 `WouldBlock`，消费已排队连接并交给 worker，再真正 join 全部 worker。断言只在 `finish()` 之后读取完整请求记录。`Drop` 也真实 join，不 detach。child 等待有 8 秒 deadline，超时会 kill/reap；测试结束后 fixture 先释放悬停响应，避免 worker 遗留。

## 剩余边界

- 本切片覆盖 CLI 的 queue list/resend/delete 和对应公共 HTTP backend；桌面 queue 调用链、登录 portal、旧 dashboard 全局路径不在本次范围。
- 共享 runner 正常每 25 ms 检查取消，观察到取消后对在途 future 最多 drain 100 ms，因此退出可能多约 100 ms；用户 `--timeout` 未被缩短，也没有 detached blocking worker。
- 没有单独模拟 DNS/TCP 连接建立阶段的系统等待；fixture 覆盖已接收请求后的头/body 悬停。
- 删除与重推的真实服务端职责、业务码和幂等性仍需脱敏生产样本确认；本测试只固定现有协议假设。真实 BOOX/OSS/neocloud、Windows/macOS 和桌面人工回归未执行。
- writer 未运行全量 `check_cli.sh`，父在 writer 结束后执行统一检查。

## 父最终集成Green

writer停止后运行 `CARGO_NET_OFFLINE=true bash scripts/check_cli.sh` 返回0：255项Rust、7项Python，0失败；新增23项统一检查1.42秒通过。fmt、严格clippy、release、依赖边界、无显示启动、打包/产物运行通过。日志 `/tmp/sandflos-cli-queue-final-check.log`。当前未提交代码的跨平台CI尚未运行；旧run不作为本次验收。
