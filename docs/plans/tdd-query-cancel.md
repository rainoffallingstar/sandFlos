# 查询请求取消：TDD 记录（2026-10-31）

基线 `3cc6fb7f29167bd722487c5fde0008c29f3f63ae`（`cli`）。改动仅在工作区，未提交、未推送，未访问真实账号、凭据、书库或生产服务，未创建 tag/release。所有 fixture 使用绑定 IPv4 loopback 的合成 HTTP 服务与合成 token。

## 范围

`HttpQueryBackend` 的 `auth_context`、`devices`、`storage`、`reading` 改为在自有的 current-thread Tokio runtime 上轮询 async future，复用既有 `cancellable_push::block_on_cancellable`。CLI 的 `query_service` 把 `cancel::FLAG` 绑定到 backend。`reading` 的 `readingInfo`、`readTimeInfo?range=week`、`dayRead` 是三个顺序只读请求；每两个之间检查取消，不启动后续请求。每个请求发出前检查取消；响应完成后再次检查，防止已完成的成功响应把取消变成成功结果或触发下一请求。

原有 blocking API 函数仍保留。新路径复用它们的状态优先信封解析：`api.rs` 提取 `devices_url`、`reading_info_url`、`read_time_info_url`、`read_time_info_params`、`day_read_url`、`day_read_params`，新旧路径使用同一构造。`cancellable_prepare.rs` 的状态优先 `envelope_json` 开放给查询模块共享。401/403、网络、业务码、无效 payload 分类、日期校验、`timezone: null` 和 endpoint 注入保持不变。错误不包含 URL、token 或响应体。

## 实际 Red

实现前先写一个公共 CLI 用例：fixture 收到 `GET /api/1/users/getDevice` 后悬停响应，CLI 使用 `--timeout 300`。测试实际记录到该请求，收到后发送 SIGINT，8 秒 child watchdog：

```bash
CARGO_NET_OFFLINE=true cargo test --locked -p sandflos-cli \
  --test query_cancel sigint_while_devices_response_is_slow -- --nocapture
```

日志 `/tmp/query-red-devices.log` 的关键失败：

```text
sandflos did not exit within 8s; stdout:  stderr:
test result: FAILED. 0 passed; 1 failed
```

这是请求已到达后的行为 Red，不是编译或 fixture setup 失败。实现最小查询路径后同一用例 0.43 秒通过，退出 130，只发出一次 users/me 和一次 getDevice，无成功输出。

## Green 与回归

新增 `crates/cli/tests/query_cancel.rs` 共 10 项，全部通过（日志 `/tmp/query-final-tests.log`）：

- 6 项真实 CLI SIGINT：devices、storage、readingInfo、week、day、以及 users/me 信封 body 半截悬停。均在 3 秒内退出 130，输出稳定 `cancelled` 错误，无结果、无 token；读取序列在取消点之后不发后续请求。
- 1 项 account 请求悬停：不发 devices。
- 1 项公开 backend 预取消：`with_cancel_flag` 已设置后调用 `QueryService::devices`，立即返回 `ServiceErrorKind::Cancelled` 且 fixture 未收到任何连接。
- 2 项公开 backend 受控顺序：fixture 收到 users/me（或 readingInfo）后，控制线程先设置取消标志，再释放成功响应；断言返回 Cancelled，且不启动 devices（或 week/day）。这直接覆盖共享 runner 的 100 ms drain 可能完成成功响应的情况。

除最初 devices Red 外，其余用例在共享 async 轮询机制就绪后首次运行即 Green；没有为其它阶段补造实现前 Red。既有查询回归通过：`failure_paths` 8 项、`query_queue_e2e` 4 项、`query_validation` 4 项（日志 `/tmp/query-regressions.log`）。严格 clippy：

```bash
CARGO_NET_OFFLINE=true cargo clippy --locked -p boox-core -p sandflos-cli \
  --all-targets -- -D warnings
```

最终 `/tmp/query-clippy-final.log` 通过，无警告。

父集成补验新增第11项：storage端点先发送成功响应头和部分信封body，再悬停；SIGINT应在3秒内退出130，无结果，无第二次storage请求。这是共享机制的直接Green回归，没有新的Red。父另移除构造器中无用的blocking client构造，保留公开构造签名，实际async client构造错误仍分类返回。上述writer日志代表10项时的候选；第11项与最终代码以 `/tmp/sandflos-cli-query-final-check.log` 的统一检查为准。子agent的Rust严格lint通过；日志同时存在本机Cargo配置文件名弃用提示，不属于Rust lint失败。

## 夹具生命周期

每个连接的头与 body 共用一个 5 秒绝对 deadline，逐次 `read` 前检查剩余时间并设置超时，写超时 5 秒。`finish()` 先置 stop/release，join listener；listener 在 stop 后循环 `accept` 到 `WouldBlock`，消费已排队连接并交给 worker，随后真正 join 全部 worker，再返回完整请求记录。断言只在该过程之后读取记录。`Drop` 也只做真实 join，不 detach。

## 剩余边界

- 本切片只覆盖 CLI `HttpQueryBackend` 的 users/me、devices、storage 和 reading 三个请求。auth 登录 portal、queue、push、OSS 上传/后写入及桌面路径不在本次改动内。
- 共享 runner 正常每 25 ms 检查取消；观察到取消后对在途 future 最多 drain 100 ms，因此退出可能有最多约 100 ms 的额外等待，且已完成响应随后被丢弃为 Cancelled。用户 `--timeout` 未被缩短，也没有 detached blocking worker。
- 测试为 Linux 公共 CLI 子进程和 loopback HTTP；Windows 不运行 Unix SIGINT 用例。未单独模拟 DNS/TCP 连接建立阶段的系统等待。
- writer未做全量测试；父在writer结束后运行统一 `check_cli.sh`，最终结果见下方集成记录。未做当前工作区的跨平台 CI、真实平台或生产验收。

## 父集成最终Green

`CARGO_NET_OFFLINE=true bash scripts/check_cli.sh` 返回0（`/tmp/sandflos-cli-query-final-check.log`）：232项Rust、7项Python，0失败；新增查询11项全部通过（0.36秒），fmt、严格clippy、release、CLI依赖边界、无显示启动、打包和产物运行全部通过。完整检查在产品writer结束及父修订后执行。双轴审查及父处置分别见 `cli-query-standards-review.md`、`cli-query-spec-review.md`，未确认本slice功能缺陷；维护判断与未测平台边界保留。
