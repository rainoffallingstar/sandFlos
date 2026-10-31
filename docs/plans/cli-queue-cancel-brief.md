# Queue取消实施与验收

2026-10-31。基线3cc6fb7加当前工作区，前5片232Rust+7Python完整检查已通过，不重做。用户明确DeepSeek V4.1 Flash high实施，父集成审查。读AGENTS、cli-handoff、cli-next-steps-and-debt。S1-S4已确认。

范围：HttpQueueReadBackend/HttpQueueMutationBackend、必要QueueService的unknown/取消结果衔接、CLI queue.rs取消标志及输出、新cancellable_queue.rs/lib私有模块、api/push必要pub(crate)协议helper共享、cancellable_push必要共享asynchelper曝光。新公共queue_cancel.rs及tdd-queue-cancel.md。不要改桌面、authportal、PushService/上传/query逻辑、manifests。单writer小patch，完成后fmt。禁止提交推送、真实账号凭据书库、tag/release。

先一个真实CLI queue list慢_changes响应测试，收到目标后SIGINT，--timeout300，8秒watchdog实际Red，再最小Green；接着逐项实现resend/delete。禁止编译/setup失败作Red，后续共享机制直接Green诚实记录。复用现有ownedasync runner，不缩短timeout、不detachblockingworker。旧blocking公开API兼容。优先共享原请求构造/解析/签名，不复制协议算法。所有新请求前检查flag，auth/syncToken/获取doc后不得开始新写入；保留已确认写成功，未确认mutation可能写入则unknown且一次请求不盲重试。标明取消与unknown同时存在时退出码策略，既有CLI总体130取消、6unknown契约按实际输出约定保持。先读queue现有result/CLI逻辑再设计。

公开tests：list auth/syncToken/_changes响应头与body悬停迅速退出130，无结果；resend准备阶段取消不得saveAndPush，发出后响应悬停快速返回unknown，不retry；delete第一步/第二步可能写入时unknown，不启动下一步骤；已经确认成功遇cancel受控竞态不变unknown。取消发生在读取doc/syncToken阶段可精确控制flag先置再放成功响应，断言无后续mutation。status-first/401业务/无效payload/404幂等、cookie/URL/参数、secret-free不回归；必要沿既有http_failure_paths/query_queue_e2e验证。不要无限加矩阵，覆盖机制/关键状态。

fixture仿最终query_cancel/pre_write_cancel：socket每次read检查绝对deadline，write有界；stop/release后drain排队连接并真正join所有线程，child kill/reap所有路径。无sleep冒充信号已观察。所有数据合成、literal loopback。只跑targeted+strictclippy，父在writer停止后全套。保存实际Red/Green原始日志；交付报告文件/计数/语义/limits。

## 父WIP集成提醒（完成前必读）

1. async resend_request不得调用会创建并block_on运行时的同步default_bucket/signed_url/write_message wrappers；曝光原async request helpers并await，避免nested runtime panic，不复制签名/轮询算法。
2. resend外层block_on取消须用future外部的write_started状态区分准备取消130与已写未确认unknown6；不能外层统一Cancelled掩盖可能写入。
3. delete只将已写入后的network/timeout/protocol无确认改unknown；明确HTTP拒绝（特别404幂等、401认证）和Business错误保留类别。成功DELETE status即baseline确认，batchDelete需完整envelope确认。
4. 原parent PUT成功还需同步确认才算完整操作；取消后不能继续发GET或fallback写入，未完成同步可unknown并说明；无parent的PUT已成功必须保留成功。
5. targeted新测试须真正验证resend async路径，不能只靠编译；文件尾duplicate函数/不匹配错误类型仅WIP，不作为行为Red。

## 父稳定候选审查修订

6. confirm_sync_after_resend取消经request_api_error映射ApiError::input再AfterSubmission会退出2，丢失已PUT未同步的unknown；取消必须Protocol/unknown6。直接pub(crate)共享原cancellable_push::confirm_sync_request并await，避免重复polling算法且保留已完成确认响应。
7. parent显式PUT404表示确定拒绝；若此时取消，尚未fallback写入，应130而不是unknown。重置write_started并guard；fallback真正发PUT时再置true，syncToken等待取消不能冒充可能写入。补公开parent同步取消/404后受控取消无fallback请求的用例。所有修订完成再报告稳定。
