# 本轮查询取消实施

基线3cc6fb7，工作区前四片已完成，221Rust+7Python全检查已通过，不重做。用户要求检查技术债/计划并继续DeepSeek V4.1 Flash high实施，允许必要重构。当前优先补HttpQueryBackend devices/storage/reading的取消。读cli-handoff、cli-state-migration、cli-review-and-tdd及AGENTS。

Own: services.rs仅HttpQueryBackend结构/构造/QueryBackend实现及必要imports；CLI query.rs绑定cancel::FLAG；新core cancellable_query.rs及lib私有mod；必要api纯URL/query构造helper与cancellable_prepare get_json pub(crate)共享。新CLI公共S1/S3测试query_cancel.rs；docs/plans/tdd-query-cancel.md。不要改PushService/OSS/Read/后写入语义、auth portal、桌面、queue、manifest。单writer，可结束后fmt。不commit/push/tag/release，不用真实账户凭据书库。

目标：--timeout300下，users/me和各查询响应头/信封body悬停均可SIGINT后<=3秒退出130；CLI稳定cancelled错误，无成功输出/秘密；reading三个连续只读请求之间及auth->query之间检查取消，不启动后续请求。取消标志可精确控制的竞态用公开backend S2/S3，不用sleep假装已观察信号。原有状态优先/业务分类/secret-free、日期编码/null/timezone:null、身份查询不附带storage和endpoint隔离契约保持。不缩短用户timeout，不启动detached blockingworkers。旧公开blocking API兼容。沿已存在owned async runner，不新增runtime方案或盲retry。

真实Red先写一个现有CLI慢devices请求用例，收到目标后SIGINT由8秒child watchdog实际失败，再最小Green；其它case逐项补，若共享机制已修复就记直接Green回归，不倒退实现造Red。fixture绝对deadline每次实际read前检查，所有连接/线程/child实际回收；断言前处理已排队连接并join，勿复用会detach的旧helper。使用合成数据。参考pre_write_cancel.rs最终夹具，避免过度铺设参数化框架。只跑targeted测试和必要clippy；父审查/全量检查后收尾。保存实际Red/Green命令/结果/限制，结束报告最终改动文件与tests计数。
