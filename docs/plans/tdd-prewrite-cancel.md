# 前置 HTTP 准备请求取消：TDD 记录（2026-10-31）

基线：`3cc6fb7f29167bd722487c5fde0008c29f3f63ae`（`cli`）。工作区未提交、未推送；未访问真实账号、凭据、书库、平台 keyring 或生产 OSS，未创建 tag/release。实现与验证只使用合成 token、临时文件、绑定 `127.0.0.1` 的 HTTP fixture。

## 范围与公共行为

`HttpPushBackend` 在推送消息写入前发出的只读准备请求现在都可被进程级 SIGINT 及时打断：

- `/api/1/users/me`（账号）。
- `/api/1/statistics/v2/user/storage`（配额）。
- `/api/1/config/buckets`（默认 bucket）。
- `/api/1/config/stss`（STS）。
- signed URL 的 `Range: bytes=0-0` OSS 校验 GET，包括原有的最多 3 次只读尝试与退避。

新的 `crates/core/src/cancellable_prepare.rs` 在自有的 current-thread Tokio runtime 上轮询这些 async future。每个请求发前和每个重试前都检查同一取消标志；观察到取消后 future 被 drop，连接随 runtime 关闭，不留下 detached blocking worker。用户 `--timeout` 原样作为单次请求超时，不被缩短或改写。共享 runner 每 25 ms 轮询一次取消；观察到取消后还会对在途 future 最多 drain 100 ms，以保留可能已经完成的成功响应。因此准备阶段确实可能延长最多约 100 ms 等待，见下方父审查修订。

原有 blocking 公共函数（`api::fetch_*_with_endpoints`、`push::build_verified_signed_download_url_with_endpoints`、core/push.rs 旧入口）保留并继续供旧调用者使用。新的 async 路径复用其 URL 构造、信封解析、身份/配额解析、bucket 选择、STS 解析和 signed URL 构造，而非复制协议：

- `api.rs` 提取 `user_me_url`、`storage_url`、`buckets_url`、`sts_url`、`parse_auth_identity`、`apply_storage_quota`、`parse_buckets`、`select_default_bucket`、`parse_oss_sts`。
- `push.rs` 将既有 `signed_download_url_with_endpoints` 与 STS 最短剩余时间常量开放为 `pub(crate)`，由新旧路径共同使用。
- `services.rs` 只替换 `HttpPushBackend::{auth,default_bucket,sts,signed_url}` 的实现；新增 `classify_request` 将取消映射为 `ServiceErrorKind::Cancelled`，其余继续走既有 `classify`（401/403、网络、business、invalid payload 分类不变）。

状态优先和脱敏保持：HTTP 非成功状态先于 body 解析；错误不含 URL、token、STS secret 或原始响应体。显式回环 endpoint 注入仍由 `ApiEndpoints` 负责，生产客户端不读取测试环境变量。

## 实际 Red

先在实现前运行最小公共 auth 用例，收到请求后 SIGINT，`--timeout 300`，fixture 悬停响应，8 秒 child watchdog：

```bash
CARGO_NET_OFFLINE=true cargo test --locked -p sandflos-cli \
  --test pre_write_cancel sigint_while_account_lookup -- --nocapture
```

当时输出见 `/tmp/prewrite-red-auth.log`，关键失败：

```text
sandflos did not exit within 8s; stdout:  stderr:
test result: FAILED. 0 passed; 1 failed
```

这不是编译或 fixture setup 失败；fixture 已记录 `GET /api/1/users/me`，子进程仍在阻塞请求中存活。实现 auth 后该用例 0.38 s 通过，退出 130，未发出 storage/bucket/STS/upload/verify 请求。

实现共享机制后为全部五个阶段补齐回归断言，并将其复制到干净的 detached 工作树（基线 `3cc6fb7`）运行，避免把修复后的 Green 反推成 Red：

```bash
git worktree add --detach /tmp/sandflos-prewrite-baseline-3cc6fb7 \
  3cc6fb7f29167bd722487c5fde0008c29f3f63ae
# 仅把新测试文件复制进去，不改基线生产代码
CARGO_NET_OFFLINE=true cargo test --locked -p sandflos-cli --test pre_write_cancel
```

`/tmp/prewrite-red-baseline.log` 记录 0 passed、5 failed，五项均在各自悬停阶段被 8 秒 watchdog 杀掉（`sandflos did not exit within 8s`）。五项覆盖 account、storage、bucket、STS 与 signed-URL Range GET。

该工作树随后被复用为格式差异整理的scratch：日志时间为00:58:58 UTC，最终api/services文件在01:01:29 UTC再次修改，收尾时已不是测试时原样。父保留了最终scratch差异 `/tmp/sandflos-prewrite-baseline-final-scratch.diff` 并回收本任务创建的临时工作树。因此上述日志仅作为实施后的辅助对照，不能据最终scratch重建测试时完整源码，也不替代最初auth实际test-first Red。这里“仅复制测试”的命令注释属于实施记录，不是父独立复原的源码哈希证据。

实现过程中另有一个失败值得记录：最初 async JSON helper 直接解析 body 而非 `{"result_code":0,"data":...}` 信封，导致 storage 阶段没有到来。修复为复用 `api::parse_envelope_with_status` 后继续。该失败是辅助实现错误，不计作产品行为 Red。

## Green

```bash
CARGO_NET_OFFLINE=true cargo test --locked -p sandflos-cli --test pre_write_cancel
```

结果：5 passed、0 failed（`/tmp/prewrite-green-all-phases.log`）。每项断言：

- SIGINT 后 3 秒内退出，进程码 130，JSON 为 `push_incomplete`。
- 后续网络阶段零请求；第二文件不开始。auth/storage/bucket/STS 阶段此前没有 OSS 上传；Verify 阶段第一个文件已经完成一次 OSS PUT，取消阻止其后的 message/confirm/save 与第二文件。
- 只有悬停请求被发出一次，无 blind retry。
- signed-URL 校验取消后不重新请求 STS、不启动 message PUT。

针对性回归（`/tmp/prewrite-regression.log`）：

```bash
CARGO_NET_OFFLINE=true cargo test --locked -p sandflos-cli \
  --test pre_write_cancel --test push_cancellation \
  --test post_write_cancel --test read_progress
```

结果 13 passed：既有上传取消、后写入取消/unknown/身份保留、连续读取进度均未回归。严格 clippy 通过：

```bash
CARGO_NET_OFFLINE=true cargo clippy --locked -p boox-core -p sandflos-cli \
  --all-targets -- -D warnings
```

## 剩余边界

- 本切片只覆盖 `HttpPushBackend` 的上述准备阶段。query/queue 的其它 blocking 读取、登录上游请求与桌面旧路径不在本切片。
- 测试是 Linux 公共 CLI 子进程与回环 HTTP；Windows 不执行 Unix SIGINT 用例。没有声称跨平台进程信号已实测。
- 8 秒 watchdog 与 3 秒 prompt-exit 是测试界限；实际退出时间取决于调度与连接关闭，Green 实测各阶段均在一秒内。
- 请求悬停覆盖响应头未到达及 API 信封 body 未读完；没有单独模拟 DNS/TCP connect 阶段的系统阻塞，也未声称该阶段可瞬间打断。
- 没有生产 OSS/BOOX 或真实平台验收；合成 STS/OSS 只验证协议路径和取消顺序。

## 共享文件说明

`services.rs` 只改 `HttpPushBackend` 的准备四方法与一个私有分类函数；upload/Read/progress 及后写入逻辑未改。`api.rs` 的提取只搬运既有逻辑，旧 blocking 函数仍调用相同 helper。`push.rs` 只扩大三个既有项的可见性：签名构造、STS 阈值与剩余时间计算。


## 父审查修订（fresh agent，2026-10-31）

本轮没有并行生产 writer；以下更正取代上文对应的事实表述。只修改了 `cancellable_prepare.rs`、`push.rs` 的 helper 可见性、`pre_write_cancel.rs` 与本文件。

**共享取消机制的实际等待。** 准备请求共用 `cancellable_push::block_on_cancellable`：正常轮询间隔 25 ms；观察到取消后还会对在途 future 最多再 drain 100 ms，以保留已完成的成功响应。初版“准备阶段不延长取消”的表述已更正。该机制本身与后写入路径一致，本切片没有缩短用户 `--timeout`，也没有 detached blocking worker。

**测试生命周期。** 原 fixture 的 `join_with_deadline` 在两秒后直接丢弃 `JoinHandle`，会 detach listener/worker。现改为：

- 每个 socket 的请求头和 body 读取共用一个 5 秒绝对 deadline。父集成将 `read_line`/`read_exact` 改为显式逐次 `read`，每次读取前检查 deadline 并设置剩余超时，避免内部多次读取延长等待；写超时 5 秒。
- `PrepareFixture::finish()` 先置 stop/release，join listener；listener 在 stop 后循环 `accept` 直到 `WouldBlock`，消费所有已排队连接并交给 worker。随后真正 join 全部 worker，再返回完整请求记录。
- 断言只在 `finish()` 之后读取该记录。`Drop` 也只做真实 join，不再静默 detach。

**新增两个公共行为用例。** `sigint_while_api_envelope_body_stalls_exits_promptly_without_followup` 让 `GET /api/1/users/me` 先发出 200 响应头和半截信封 body，再悬停余下 body；child 收到该阶段信号后 SIGINT，`--timeout 300`，3 秒内退出 130，之后无 storage/bucket/STS/upload/verify，第二文件不开始。该用例首次即 Green，记录为共享 `block_on_cancellable` 机制的回归验证，不声称是新增实现 Red。

`cancelled_account_response_does_not_start_storage` 通过公开 `HttpPushBackend::with_cancel_flag` 与 `PushBackend::auth_context`（S2/S3）悬停 users/me。控制线程先设置取消标志，再立即释放成功响应，精确建立顺序；断言 Cancelled 且不发 storage。父集成用它替换依赖 SIGINT 后 50 ms sleep 的用例，避免把调度延迟当作已观察取消的证据。其余六项是真实 CLI SIGINT 用例；没有修改后写入确认成功优先的 runner。新增覆盖均为共享机制回归，不补造 Red。

**证据归类更正。** 实际 test-first Red 只有最小公共 auth 用例：`/tmp/prewrite-red-auth.log` 中 fixture 已记录 `GET /api/1/users/me`，8 秒 watchdog 超时退出，`--timeout 300` 排除了配置超时。实现 auth 后该用例 Green；storage/bucket/STS/Verify 用例随后补上，首次运行即 Green，属于同一 future 轮询机制的直接回归验证。把测试复制到基线 `3cc6fb7` 的 detached 工作树得到 5 failures，是**实现完成后**的对照实验，不能写成五个独立用例各自 test-first。`storage` 早期“never reached”失败是 async helper 未解 API 信封的中间协议回归，不是慢 storage 取消的行为 Red。

**Verify 阶段的真实请求顺序。** signed-URL Range GET 悬停用例中，第一个文件在此前已经完成一次 OSS PUT；取消阻止的是该文件的 message PUT、后续 confirm/save，以及第二个文件的全部上传。初版“两个文件都不进入 OSS 上传”的表述只适用于 auth/storage/bucket/STS 的前置阶段断言，不适用于 Verify 阶段。Range 仍按 headers-only 契约在 200/206 响应头到达时接受，不为测试等待 body。

**helper 共享。** `cancellable_prepare.rs` 不再复制 `sts_remaining_seconds`，改为使用 `push.rs` 中 `pub(crate)` 的同一函数；重复的 chrono 导入已删除。签名构造、credential trim/拒空、状态优先信封解析保持不变。

## 修订后的命令与计数

```bash
CARGO_NET_OFFLINE=true cargo test --locked -p sandflos-cli \
  --test pre_write_cancel -- --test-threads=1
# 7 passed; 0 failed

CARGO_NET_OFFLINE=true cargo test --locked -p sandflos-cli \
  --test pre_write_cancel --test push_cancellation \
  --test post_write_cancel --test read_progress -- --test-threads=1
# 7 + 2 + 5 + 1 = 15 passed; 0 failed

CARGO_NET_OFFLINE=true cargo clippy --locked -p boox-core -p sandflos-cli \
  --test pre_write_cancel -- -D warnings
# Finished, no warnings
```

7 个 pre-write 用例为 6 项 CLI SIGINT（auth、storage、bucket、STS、Range verify、API 信封 body stall）及 1 项公开 backend 受控取消/成功响应竞态。全部断言在 `finish()` 真实 join/drain 后检查完整请求记录。

父最终集成重新运行 `CARGO_NET_OFFLINE=true cargo test --locked -p sandflos-cli --test pre_write_cancel`：7 passed、0 failed，耗时 0.35 s，日志 `/tmp/prewrite-parent-integration.log`。这是最终逐次 read/受控取消竞态版本的 Green；没有补造新的 Red。
