# CLI 推送、队列与查询服务实施记录

日期：2026-10-29。公共边界为 `cli-review-and-tdd.md` 的 S1/S2/S3：服务通过注入的 `PushBackend`、`QueueReadBackend`、`QueueMutationBackend`、`QueryBackend` 测试；CLI 进程测试使用临时配置目录和本地文件，不访问真实账号或云端。

## 已实现的行为

- `PushService::push_files` 返回逐文件 `PushReport`。每项保留路径、显示名、阶段、消息 ID、rev、资源键、大小和类型化错误；进度通过 `PushProgress` 回调提供，核心不打印文本。
- 空批次、不可读文件、目录、0 字节文件、超过 200 MiB 的文件及超出剩余配额的计划在调用认证、STS 或 OSS 之前失败。混合批次保留输入顺序，合法项继续处理。
- 每个合法文件依次执行 STS、OSS 流式 PUT、签名下载地址、写入 neocloud 消息、等待同步 rev、`saveAndPush`。只读阶段失败为 `failed`；同步确认或最终保存结果无法确认时为 `unknown`，不会报告成功。
- 退出码按结果汇总：全失败按认证、网络、本地 I/O、输入的优先级选择；部分成功或任何未知结果为 6。云端接受是服务端已接收推送请求，不代表 BOOX 设备已经下载。
- `QueueService` 查询不刷新桌面仪表盘，默认最多返回现有 30 条上游窗口并标记 `truncated`、`upstream_window`。非法或空白 ID 在认证前拒绝。
- 队列重推和删除不盲目重试。删除要求显式确认；远端文档 404 与消息记录 404 均按幂等成功处理；第二步失败返回结构化部分完成/未知结果，CLI 以 6 退出，保留 `complete=false` 数据。
- `QueryService` 的设备、存储、阅读统计分别调用，设备列表不触发阅读指标查询；日期在请求前严格校验为真实存在的 `YYYY-MM-DD`。
- HTTP 适配器位于服务边界之外。401/403 映射认证错误，404 在队列操作中映射为 `NotFound`，网络超时映射网络错误，其余服务端故障不把响应正文当作稳定业务码。
- 取消在本地预检、OSS 流式读取与各阶段间检查，批次不再启动下一个文件。消息已创建但尚未提交，或最终提交未收到确认时取消，结果按 `unknown` 保留已知消息 ID。最终提交已收到成功确认则保留 accepted；不会因为晚到的取消信号声称它未完成。blocking 网络调用仍受有限请求超时约束。

## 本次收尾的 Red/Green 与回归

接手时服务实现和大部分测试已在未提交工作区；没有把这些已有测试冒充为本次 Red。收尾审查发现并修复两个具体缺陷：

1. 队列删除的 HTTP 适配器把所有 404 归为网络错误，导致服务的幂等删除分支永远无法通过真实适配器到达。新增 `queue_not_found_is_classified_for_idempotent_delete` 固定 `queue.delete` 的 404 分类；首次运行前分类实现与测试同批加入，故该测试是缺陷回归覆盖，不记录为先行 Red。
2. CLI 对 `QueueMutationResult { complete: false }` 仍输出成功并退出 0。新增 `incomplete_delete_is_exit_6_and_keeps_structured_result` 与 `complete_delete_is_success`，并把部分结果映射为稳定代码 `queue_delete_incomplete`、退出 6 和结构化 `data`。

同时补充了此前未覆盖的服务契约：完整推送的阶段顺序、同步未确认时保留消息 ID 和 `unknown`、超过 200 MiB 在认证前拒绝、计划空间恰好等于剩余配额时允许上传。

## Green 结果

```bash
cargo test --locked -p boox-core services::tests
cargo test --locked -p sandflos-cli --bin sandflos
```

- 服务模块 18 项测试通过，0 failed；CLI 二进制单元测试 2 项通过，0 failed。
- 完整 `cargo test --locked -p boox-core -p sandflos-cli` 中，core unit、书库集成、CLI auth/process/push-queue 均通过；具体总数以最终集成记录为准。
- `cargo clippy --locked -p boox-core -p sandflos-cli --all-targets -- -D warnings`：通过。
- `cargo fmt -p boox-core -p sandflos-cli -- --check`：通过。
- `bash scripts/check_cli.sh`：覆盖 fmt、两 crate 测试、CLI lint、release 构建、依赖闭包和隐藏显示环境 smoke；取消切片补齐后该命令再次通过。
- 取消回归：`cancelled_upload_stops_the_batch_before_any_message_write`、`cancellation_after_the_first_file_keeps_that_acceptance`、`cancellation_after_message_creation_is_unknown_with_the_message_id`（S2）；`sigint_during_upload_exits_130_without_creating_a_message`（S1/S3，真实子进程 SIGINT、慢速回环 OSS）。

## 未验证边界

- 没有访问真实 BOOX 账号、OSS、neocloud、设备或生产队列；没有执行真实推送、重推或删除。
- 没有做 200 MiB 文件的真实网络传输、慢链路或真实连接中断 smoke；取消使用回环慢速夹具和真实 SIGINT 验证，不证明生产链路的实际吞吐。
- HTTP 适配器没有用真实协议样本验证所有业务码、429/5xx 重试语义和响应丢失后的幂等性。未知结果要求先查队列再决定重推。
- macOS/Windows 的凭据后端、防火墙和设备发现未在本机验证。
