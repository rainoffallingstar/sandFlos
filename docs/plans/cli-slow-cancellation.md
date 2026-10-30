# 慢上传背压下的 SIGINT 取消

日期：2026-10-30。范围是 `crates/cli/tests/slow_cancel_backpressure.rs`、`crates/core/src/services.rs` 的 OSS PUT 边界和本文件。没有修改 `push.rs`。未访问真实 OSS、neocloud 或账号；夹具使用合成 STS、消息和响应。

## 问题与真实 Red

此前 `HttpPushBackend::upload` 使用阻塞 `reqwest::blocking`。夹具接受 OSS PUT、读取约 8 KiB 前缀后停止读取，客户端的 HTTP 与内核发送缓冲随之填满。此时阻塞发送线程停在 socket 写，`CancellableReader` 没有再被轮询，单独的原子取消标志无法让请求返回；客户端只能等配置的 HTTP 超时。

回归用例固定 8 MiB 稀疏文件、两个输入文件和 `--timeout 300`。夹具使用字面回环 `127.0.0.1`，连接建立后设置 1 秒读写超时；等待上传开始时最多 5 秒，子进程 watchdog 为 10 秒，SIGINT 后要求 3 秒内退出并 kill/reap 超时子进程。先用旧实现运行：

```bash
CARGO_NET_OFFLINE=true cargo test -p sandflos-cli --test slow_cancel_backpressure -- --nocapture
```

实际 Red：测试运行 12.22 秒，子进程在 10 秒 watchdog 到期时仍未退出，被 kill 后报 `sandflos did not exit within 10s`。这不是编译或夹具错误；客户端确实停在已背压的上传请求中。

## 实现

只替换 `HttpPushBackend::upload` 的 OSS PUT 边界：

- 每次上传创建 owned 的 current-thread Tokio runtime 和 async `reqwest::Client`，不依赖调用方的全局 runtime。
- `CancellableUploadBody` 按 64 KiB 块读取文件，`size_hint` 返回精确剩余长度；请求仍显式设置原文件 `Content-Length`。读到文件提前结束时返回错误。
- 请求 future 由 `tokio::pin!` 固定，以 25 ms 超时轮询。取消标志命中时退出轮询并丢弃 future，连接随请求一起关闭。
- 响应状态、请求 future 和 client 都在 runtime 仍存活时先释放，然后才 drop runtime；没有分离的上传 worker。
- 配置的 `--timeout` 原样作为 async client 总超时，没有通过缩短用户超时来让测试通过。
- STS 过期检查、OSS 签名、状态码分类、确认前检查和其他 HTTP 阶段保持原有逻辑。

新增 `tokio`（`rt`、`time`）、`bytes`、`http-body` 直接依赖，并更新 `Cargo.lock`；未启用下载式 `stream` feature。没有修改生产 CLI 参数或其他服务阶段。

## Green 与验证

修复后同一命令实际 Green，测试套件报告 2 passed、0 failed，耗时 1.68 秒（SIGINT 用例内含发送前 1.5 秒等待，因此 SIGINT 后退出满足 3 秒上限）。同一夹具另有无 SIGINT 对照：`--timeout 1` 的背压上传仍以退出码 4、`upload_oss` / `network` 结束，证明没有通过缩短或绕过用户超时让取消用例通过。SIGINT 断言覆盖：

- 退出码 130，JSON `error.code` 为 `push_incomplete`；
- 第一项为 `upload_oss` / `cancelled`，第二项为 `prepare` / `cancelled`；
- 仅一次 OSS PUT，零次 neocloud 消息 PUT、零次 `POST /api/1/push/saveAndPush`；
- watchdog 超时会 kill 并 reap，不在测试结束时留下子进程。

另运行：

```bash
CARGO_NET_OFFLINE=true cargo test -p boox-core --test push_cancellation --test transfer_streaming --test transfer_contract --offline
CARGO_NET_OFFLINE=true cargo test -p sandflos-cli --test push_cancellation --test slow_cancel_backpressure --test http_output_secrets --offline
CARGO_NET_OFFLINE=true cargo clippy --locked -p boox-core -p sandflos-cli --all-targets --offline -- -D warnings
CARGO_NET_OFFLINE=true cargo fmt -p boox-core -p sandflos-cli -- --check
```

结果为 core 取消 6 项、传输契约 1 项、流式上传 2 项、CLI 取消 2 项、背压取消/超时对照 2 项、输出/进度 3 项全部通过；clippy 与格式检查通过。另运行完整的 `cargo test --locked -p boox-core -p sandflos-cli --offline`，共 198 项测试通过、0 失败。精确上传字节/长度仍由既有 `transfer_streaming` 和 `http_output_secrets` 夹具验证。

## 剩余边界

- 这次只让 OSS PUT 的系统等待可被取消。`write_message`、`confirm_sync` 和 `saveAndPush` 仍使用阻塞 client；SIGINT 只能在这些调用之间或超时返回后被观察到。尤其已发出 `saveAndPush` 但响应丢失时，客户端仍可能等到配置超时才报告 unknown，不能声称服务端未提交。
- 夹具是 8 MiB、字面回环和合成凭据，不证明 64 MiB、真实慢链路、生产 OSS 或平台行为。它证明的是同一背压机制：请求停在 socket 可写等待时，原子标志仍能打断上传。
- 每次 OSS PUT 新建 runtime/client 有少量开销；没有测量生产吞吐。异步 body 的内存上界是单个 64 KiB 块及网络栈自身缓冲，不是整个文件。
- 25 ms 轮询决定观察到取消的粒度；在本次本机夹具中满足 3 秒上限，但没有跨平台延迟承诺。
- body 每次同步读取最多 64 KiB 本地文件；若磁盘或网络挂载本身阻塞，运行时仍可能等待该次文件读取。这次修复针对 socket 写背压，不承诺打断任意文件系统调用。
