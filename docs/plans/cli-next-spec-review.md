# CLI 下一轮 Spec 轴审查

日期：2026-10-30。基线：`406ef53720dabf1df412359558dc8b8bc2002940`。范围：工作区与暂存区 diff、T7/S1–S4、`cli-handoff.md` 未完成项。只读审查；本文件外未修改文件，未提交。

## 通过项

基准按 `cli-upload-performance.md:7` 的要求校验了精确字节数、SHA-256、`Content-Length`、单次 OSS PUT，并另验一次消息 PUT 和 `saveAndPush`；`docs/cli.md:58` 明确大型基准为手动、非例行的 `scripts/check_cli.sh` 负担。`VmHWM` 在 `:15` 标注为约 1 ms 采样，且明确最后一次采样间隔可能漏掉峰值。fixture 使用 literal loopback，接受连接有有限 socket deadline。状态文档明确是迁移评估，未声称已实现 app-owned `RuntimeState`。

实际运行：

```text
cargo test --locked -p sandflos-cli --test slow_cancel_backpressure
1 passed
cargo clippy --locked -p sandflos-cli --all-targets -- -D warnings
通过
```

## 发现

1. **缺失明确要求的取消证据文件。**任务要求“Write `docs/plans/cli-slow-cancellation.md`; describe remaining cancellation limits for other blocking stages.”该文件不存在。当前 `services.rs:1311-1352` 只为 OSS PUT 建立异步运行时；消息写入、同步和 `saveAndPush` 仍走 blocking 路径，其剩余取消上限没有被集中记录。T7 要求“慢服务、连接中断、重复读不超总量、STS 到期、取消释放本地占用”（`cli-review-and-tdd.md:140`），这些结果与界限需要该证据文件承载。

2. **取消文档仍在描述已经被替换的 reader 实现。**`cli-handoff.md:25` 写“SIGINT 使用可取消 reader”，`:62` 写“v1 取消依赖 reader……不能保证瞬间打断已阻塞的系统调用”；`tdd-transfer.md:7` 仍称 `CancellableReader`，`:42` 仍称 blocking reqwest 无法中断。实际代码已改为 `CancellableUploadBody` 加 owned Tokio runtime、25 ms 轮询取消并 drop future（`services.rs:1311-1352`）。这些文字与代码和 300 秒超时回归相反，会误导验收者。规范要求 SIGINT“退出 130，并停止后续副作用”（`cli-review-and-tdd.md:97`）；当前测试虽通过，但文档没有按新边界记录其适用范围。

3. **缺少该修复的行为 Red 记录。**任务要求“Capture actual behavioral Red before code fix (current reader alone blocks).”现有 `tdd-transfer.md` 只记录旧 reader 和旧 64 MiB 实验，没有本轮 slow-backpressure 测试在旧实现下阻塞、并在新实现下通过的可复核记录。现有绿色测试本身通过，但未满足诚实保留 Red/Green 证据的要求。

未发现额外范围扩张：生产改动限于 `services.rs` 的上传边界及必要依赖；`push.rs` 未改。

## 父 agent 处置

1. 已补 `cli-slow-cancellation.md`：实际旧实现 Red 为 300 秒请求超时下 SIGINT 后 10 秒 watchdog 仍未退出；新实现 Green。新增无 SIGINT 的 1 秒超时对照保留原超时契约。其他阻塞 HTTP 和本地文件读取限制已列出。
2. 已修：交班记录新增本轮区段，旧验收明确标为历史；`tdd-transfer.md` 顶部指向当前异步 OSS 实现，逐项状态更新本机 200 MiB 测量和剩余范围。
3. 已补真实行为记录，未将先前夹具编译问题或已有行为直接 Green 冒充 Red。

方案轴 3 项均已处置。桌面状态迁移仍只是评估，真实账号、平台和生产性能验收未完成。
