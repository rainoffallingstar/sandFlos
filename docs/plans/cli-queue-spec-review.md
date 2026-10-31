# Queue 取消方案轴审查

范围：基线 3cc6fb7 加工作区，仅 queue 增量。静态审查，未跑测试、未改产品。

**确认缺陷（1）**

- `docs/plans/tdd-queue-cancel.md` 不存在。`cli-queue-cancel-brief.md` 明确要求该文件交付实际 Red/Green 原始日志、文件/计数/语义/limits；因此无法核验“真实行为 Red 而非编译/setup 失败”、targeted 计数及每阶段取消证据。现有 `crates/cli/tests/queue_cancel.rs` 虽覆盖 list 准备/响应、resend syncToken/PUT 头与 body 悬停、delete 两阶段、确认成功竞态等，测试存在不能替代任务要求的证据记录。

**维护判断**

- 阶段边界与结果映射与方案一致：`cancellable_queue.rs` 在每轮请求前调用 `check_before_request`；`write_started` 仅在 PUT/DELETE/batchDelete 发送前置位，准备期取消映射 130，可能写入后取消映射 unknown，已确认响应优先成功（见 `resend`、`flatten_delete_result`、`QueueService::delete` 及 queue_cancel.rs 对应断言）。
- resend 未盲重试；parent 的消息 PUT 仅在提交响应明确 404 时触发 fallback，确认 polling 也有取消检查。delete 的 404 幂等与 401/业务错误保留类别；cookie、URL helper 与状态优先解析沿用共享代码。
- 未验证边界：未复跑 fixtures，未穷尽核对每个 fixture 的绝对 read deadline、drain/join/child reap 实现；缺 TDD 文档也使实际 Red/Green 与资源回收证据不可审计。父负责集成处置。

## 父集成处置

该报告写于writer尚在收尾时，缺TDD文件为当时交付缺口，现已由tdd-queue-cancel.md关闭。父另发现并修复：async调用同步runtime包装器；parent同步取消被归为Input；明确404后fallback写入标记未重置。最终改为await共享async helper、同步取消unknown6、404后重置并在fallback真正PUT前重新标记。23项公共测试覆盖修订结果，旧21项回归与strictclippy通过；整体Green以最终统一检查为准。报告原“阶段映射一致”不是对所有parent分支的穷尽证明。
