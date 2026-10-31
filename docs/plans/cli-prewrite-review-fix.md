# 前置取消专项修订（fresh agent）

第一writer已停止，当前五个phase取消测试Green，真实最初auth Red及后续Green日志在 `/tmp/prewrite-*.log`，规范/方案报告在 `cli-prewrite-{standards,spec}-review.md`。沿用 S1/S3；不要重做实现或基线工作树实验，不制造Red。必须逐项完成并报告：

1. `pre_write_cancel.rs::join_with_deadline`仍在两秒未结束时drop JoinHandle，是detach。消除该路径，stop/release后实际join listener与所有workers。自身socket读写必须有界，head需绝对deadline，不能通过延长每次read无限等待。已有每连接并发fixture可保留；让child退出后的所有排队请求也被accept/record（或等价drain机制），真正join后再断言无后续请求。不要只在server hold/未处理排队连接时断言0。可通过finish/drain方法消费fixture并返回完整请求记录，listener在stop时drain已排队连接直到WouldBlock后退出，再join全部worker。
2. 新增一个公开CLI API信封body stall测试：已发200响应头及部分body，余body保持不发，收到该阶段信号后SIGINT，--timeout300，<=3s exit130、无后续阶段/第二文件。不要将Range改成等body；Range200/206仍headers-only。新增测试如直接Green，诚实记录共享机制回归，勿回退实现造Red。
3. 新增一个SIGINT后立即释放held users/me响应的公开竞态测试；确保取消被CLI signal handler观察（可用现有进程信号顺序+短有界同步方法），然后响应成功也不能发storage。不要靠服务端不accept掩盖后续请求。严格不要改变后写入已确认成功优先的runner行为。
4. 复用push.rs::sts_remaining_seconds（必要pub(crate) exposure），删cancellable_prepare.rs重复函数/chrono imports。签名已共享、credential已trim/reject，保留。
5. 修正文档事实：shared runner实际有25ms轮询+最多100ms drain，不要声称准备阶段没有延长等待。Verify阶段在取消前第一文件已经有一次OSS PUT，不要声称全部测试两个文件都不上传。记录先auth实际Red→实现Green、随后其它case共享机制回归，独立baseline五失败是实现后的对照实验，不能写成新增每项均test-first。storage never-reached是中间协议回归。删关于auth.rs不明空白修改的猜测；本轮无并行生产writer，父已持有snapshot可核对。

只运行本文件的新测试及相关pre/post/read/push回归、fmt/clippy必要针对性检查，不运行full script。不要重构services postwrite错误mapping：规范报告的该点是已有维护判断，不是本切片必须修。保留原始报告，TDD文档追加具体处置和命令/计数。停止后父统一检查。

竞态测试可靠性补充：kill返回不等于ctrlc线程已置flag；不要用任意sleep假装证明已观察取消。允许在同一integration test文件通过公开`HttpPushBackend::with_cancel_flag`与`PushBackend::auth_context`（已确认S2/S3）精确控制AtomicBool，置flag后立即释放成功响应，断言Cancelled及无storage；原有五个真实CLI SIGINT案例继续证明S1信号接线。若选此方式，文档明确这是受控取消标志竞态测试，不能冒称新的CLI SIGINT竞态实测。无需新公共接口或私有helper测试。

绝对deadline实现补充：仅在一次`BufRead::read_line`或`Read::read_exact`前设置remaining socket timeout仍不是真正绝对deadline，因为它们内部可多次recv，每次timeout重置。fixture读取head/body需显式read循环，每次实际read前检查Instant deadline并设remaining，行可逐字节读/累计上限；body可分块读。这样join有实际I/O上限。
