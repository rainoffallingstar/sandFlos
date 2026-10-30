# T7 传输与取消收尾记录

日期：2026-10-30。范围是本轮取消切片：`crates/cli/tests/push_cancellation.rs`、`crates/core/tests/push_cancellation.rs`、`crates/core/src/services.rs` 的取消相关 hunk，以及本文件。规范按 `cli-review-and-tdd.md` 的 S1/S2/S3 边界；既有实现的回归测试不冒充新的 Red。

## 取消语义

- 批次在认证、每个文件开始、消息写入响应之后和提交之后检查取消。`CancellableReader` 在上传 body 读取期间检查进程标志；命中时返回 `io::Error::other`，避免 `Interrupted` 被读取适配器重试。
- OSS 确认前取消：该文件为 `upload_oss` / `failed` / `cancelled`，后续文件不进入上传、消息写入或提交。
- 写入 neocloud 消息的响应已到达后取消：保留响应的 `(id, rev)`，把该项记为 `write_message` / `unknown` / `cancelled`，不进入 `confirm_sync` 或 `saveAndPush`。
- `saveAndPush` 已开始后取消：已提交但未确认响应时为 `save_and_push` / `unknown` / `cancelled`，保留消息 ID；已完成且服务端已确认的提交仍保留为 accepted。批次不再启动下一个文件。
- CLI 有取消错误时优先退出 130，JSON 报告的 `error.code` 为 `push_incomplete`。

## Red/Green 与直接 Green

该子 agent 切片的实际 Red 为 `cancellation_after_write_response_keeps_known_message_id_and_rev`（S3，真实 `HttpPushBackend` + 回环 HTTP 夹具）。首次运行在“known message id must survive”断言失败：`HttpPushBackend::write_message` 收到成功响应后，因取消标志已置位而丢弃 `(id, rev)` 并返回 `cancelled`。最小修复是让适配器返回成功响应的身份；`PushService` 记录该身份后，在下一网络阶段前停止。修复后该项通过。夹具先读请求体再置取消位并返回 `201 {"id":...,"rev":"1-fixture"}`，服务端请求计数证明只发生一次消息 PUT、零次 `confirm_sync` GET 和零次 `saveAndPush`。

父 agent 随后新增 `preexisting_cancellation_takes_priority_over_local_preflight_errors`：先 Red，已有取消信号的两个缺失文件仍返回本地 I/O / 退出 5；预检在每个文件开始时和进入认证前检查取消后 Green，返回两项保留原路径的 cancelled / 退出 130。CLI 的凭据解析前预检传入同一进程取消信号，避免只修到用例层。日志 `/tmp/sandflos-preflight-cancel-{red,green}.log`。另修复最终成功响应被晚到取消覆盖的问题，实际 Red/Green 见 `tdd-http-errors.md`；两项真实 SIGINT 用例也补上第二项必须指向 `second.pdf` 的断言。

以下为新增回归覆盖，首次运行即通过，不宣称先行 Red：

- `sigint_during_large_upload_stops_two_file_batch_before_message_creation`（CLI S1/S3）：8 MiB 稀疏文件、慢速回环 OSS/API 夹具、真实子进程 SIGINT。断言退出 130、两项结果、首项 `upload_oss`/`cancelled`、次项 `prepare`/`cancelled`、仅一次 OSS PUT、零次 neocloud PUT / `saveAndPush`。Linux 下另读 `/proc/<pid>/io` 的 `rchar`，确认 SIGINT 前未读完整文件；服务端收到字节也少于文件大小。
- `sigint_during_save_and_push_reports_unknown_and_stops_the_second_file`（CLI S1/S3）：服务端收到且解码 `saveAndPush` 请求后暂不响应，测试发送真实 SIGINT，再释放并丢弃响应。断言退出 130、首项 `save_and_push`/`unknown`/`cancelled` 且消息 ID 与请求中的 `cbMsg.id` 一致，次项 `prepare`/`failed`/`cancelled`，仅一次上传和一次提交。
- `cancellation_stops_before_authentication_or_upload`、`cancelled_upload_stops_the_batch_before_any_message_write`、`cancellation_after_message_creation_is_unknown_with_the_message_id`、`cancellation_after_the_first_file_keeps_that_acceptance`（S2）：沿用并补强原有服务测试；每个用例使用独立取消标志，避免并行测试互相重置。
- 核心取消测试的临时文件改为 `TempTree` RAII 目录，断言 panic 时也会删除。

## Green 结果

```bash
cargo test -p boox-core --test push_cancellation
cargo test -p sandflos-cli --test push_cancellation
```

- core `push_cancellation`：子 agent 聚焦验收 5 项；父 agent 加入预检取消用例后，最终统一检查 6 项通过，0 failed。
- CLI `push_cancellation`：2 项通过，0 failed。
- 夹具边界：listener 为非阻塞 accept；连接切回阻塞并设 5 秒读写超时；慢 body 循环同时受 `Instant` 截止时间和停止标志约束；子进程等待有 10 秒 watchdog，超时会 kill 并 reap；fixture 线程 join 有 2 秒上限，断言失败不会由阻塞 socket 卡住测试进程。
- 一次 64 MiB 扩展实验发现慢夹具（约 80 KB/s）会在 SIGINT 后让客户端长时间阻塞于 socket 写。最终用例改用要求的 8 MiB，并把夹具排空速率提高到约 2.5 MB/s；这不改变生产取消语义，也不证明真实慢链路。

## 未验证边界

- 未访问真实 OSS、neocloud 或 BOOX 设备；夹具使用合成 STS、消息和响应。
- 未测量 200 MiB 文件的实际峰值内存、吞吐或生产慢链路行为；8 MiB 稀疏文件只证明本机 fixture 下可在读完大文件前中断。
- blocking reqwest 的系统调用不能被 async 瞬间打断；取消依赖 body reader 的检查、有限请求超时和阶段/批次检查。`saveAndPush` 已发出但响应丢失时，只能报告 unknown，不能声称服务端未提交。
- 子 agent 交付时只运行上述聚焦测试。父 agent 的最终完整测试、格式、clippy 和 release 验证见交班记录。
