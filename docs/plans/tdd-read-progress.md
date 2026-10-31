# 连续文件读取进度（S2/S3）

日期：2026-10-30。实现范围为 `crates/core/src/services.rs` 中的 `PushProgress`、`PushBackend` 默认观察方法、`PushService` 上传分派、`CancellableUploadBody` 计数和 `HttpPushBackend::upload` 边界，以及 `crates/cli/src/push.rs` 的人类可读渲染。没有修改上传成功后的消息写入、同步或提交逻辑，也没有改变 OSS 请求内容、重试次数或用户超时。所有夹具均为回环合成服务，没有访问真实账号、OSS 或书库。

## 行为 Red

现有 CLI 只在上传返回后打印 `Uploaded`。回归用例 `crates/cli/tests/read_progress.rs` 使用 1 MiB 文件，让回环 OSS 夹具完整读取 PUT body 后暂不发送 HTTP 响应：

```bash
CARGO_NET_OFFLINE=true cargo test -p sandflos-cli --test read_progress -- --nocapture
```

旧实现真实失败，而不是编译或夹具失败：夹具已收到完整 1 MiB，stderr 只出现 `FileStarted` 和 `UploadOss`，3 秒内没有读取进度，测试报 `no continuous read progress was reported while the OSS response was withheld`。这证明旧进度无法区分“文件已读完”和“OSS 已确认”。

## 实现

- 新增 `PushProgress::Read { index, total, path, bytes, total_bytes }`，文档和 CLI 文本明确为本地读取，不表示网络或 OSS 接受。
- `PushBackend` 保留原 `upload`，新增带默认实现的 `upload_with_progress(..., &mut dyn FnMut(u64))`。旧实现者无需修改即可编译，现有 fake 后端继续使用默认方法。
- `PushService` 调用观察方法，只转发严格递增且不超过该文件大小的样本；成功后仍单独发送一次 `Uploaded`，失败或取消路径不会发送 `Uploaded` 或 `Accepted`。
- `CancellableUploadBody` 在每次成功同步读取后递增每请求的原子字节计数。异步请求 future 的既有 25 ms 轮询读取该计数并调用调用方闭包；闭包在当前上传线程执行，不要求 `'static`，没有分离的上传 worker。
- 请求结束或错误后补发最后一次未报告的计数，随后才分类返回。一旦观察到取消，就丢弃 future 并且抑制最后样本；取消路径不会在取消开始后再次调用进度闭包。
- CLI 将事件渲染为 `read <bytes>/<total_bytes> bytes`；原有 `uploaded <bytes> bytes` 仍只在 OSS 返回成功后出现。

## Green 与证据

同一 CLI 命令在实现后通过。测试解析 stderr 中的每一个 `read <bytes>/<total> bytes` 样本，不再假定首个样本等于文件大小；它验证样本严格递增、全部不超过 1 MiB、最终样本恰为 `1048576/1048576`，并等待最终样本后才断言此前没有 `uploaded` 或 `accepted`。释放响应后退出码为 0，出现 `uploaded 1048576 bytes`，stdout 有一次 `accepted`，夹具只收到一次 OSS PUT 和一次 `saveAndPush`。

公开边界回归测试：

```bash
CARGO_NET_OFFLINE=true cargo test -p boox-core --test transfer_streaming --offline -- --test-threads=1
```

结果为 6 passed、0 failed。新增的慢读取用例发送 16 MiB body，回环服务每次只读 64 KiB 并暂停 10 ms，在真实 public `HttpPushBackend::upload_with_progress` 边界上收到多个不同样本，验证严格递增、上限正确、最终值精确等于文件长度且只有一个 PUT。另有用例在首样本后设置取消标志，证明取消错误优先于伪成功、之后没有进度闭包执行、没有最终完成样本，也没有重试。

生产 OSS 签名回归单独由公开真实 HTTP 用例覆盖。夹具记录实际 PUT 的 Date、Content-Type、临时安全令牌和 Authorization 头；测试按 OSS v1 规则独立构造签名串，包含请求方法、空 Content-MD5、Content-Type、Date、安全令牌头和 canonical resource，并用独立 HMAC-SHA1 密钥计算期望值，绝不调用生产 `sign_oss`。修复前 `upload_bytes` 的签名串缺少安全令牌头后的换行和 canonical resource；该断言会失败。修复后 Authorization 精确匹配。原 legacy `core/push.rs` 的签名形状也包含该换行和 resource。

`crates/cli/tests/http_output_secrets.rs` 的公开 `PushService` 用例同时覆盖响应丢失与正常确认：两种路径都能观察到有界的本地读取；响应丢失时 `Uploaded` 为 0、`accepted` 为 0，成功路径才各为 1。读取完成本身没有制造接受结果。

## 剩余边界

- 25 ms 是进度采样的上限间隔，不保证每隔 25 ms 必有新样本。回环快速上传仍可能只产生一个最终样本；连续多样本由 16 MiB、慢读取的公开 HTTP 用例证明。实现保证单调、有界和成功路径的最终值正确，不承诺固定刷新频率。
- 读取计数在 body future 被轮询时更新；如果本地文件系统本身阻塞单次最多 64 KiB 的读取，计数和取消观察都会等待该次读取。内存上限仍是单块加网络栈缓冲。
- 只覆盖 `HttpPushBackend` 的 OSS PUT。其他 backend 若继续使用默认观察方法，不会产生连续读取事件；它们的兼容性已保留。
- 测试证明合成回环中的事件时序和单次请求，不证明生产网络吞吐、真实 OSS 行为或桌面 UI 已迁移到这些事件。桌面迁移仍按 `cli-state-migration.md` 后续处理。
- 本轮修复后的定向证据包括 `transfer_streaming` 6 项、`push_cancellation` 6 项和 CLI `read_progress` 1 项；相邻的 CLI 输出 3 项、背压 2 项和写后取消 5 项也通过；`cargo clippy --locked -p boox-core -p sandflos-cli --all-targets --offline -- -D warnings` 通过。没有运行完整测试集，因为其他 writer 仍可能修改共享文件。本任务的 `transfer_streaming.rs` 和 `read_progress.rs` 单独通过 rustfmt；没有对共享的生产文件执行 `cargo fmt`。
