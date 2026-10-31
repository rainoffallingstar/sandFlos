# Queue取消集成审查

基线3cc6fb7f29167bd722487c5fde0008c29f3f63ae加working/untracked，commitlist为空。diff命令 git diff 3cc6fb7 -- crates/core/src/services.rs crates/core/src/api.rs crates/core/src/push.rs crates/core/src/cancellable_push.rs crates/core/src/lib.rs crates/cli/src/queue.rs，并直接读取新cancellable_queue.rs、queue_cancel.rs、tdd-queue-cancel.md。仅本次queue增量，前5片已有232Rust+7Python通过。起点快照 /tmp/sandflos-queue-review-base 可对比，勿重复报告旧片。

方案源cli-queue-cancel-brief.md及cli-review-and-tdd.md的S1-S4/T8；规范源AGENTS共同约束、README core业务/UI呈现分工。两个reviewer只做各自轴，最多6读/toolcall后写<=400字报告，即结束，不跑测试/改产品/commit/push。

规范轴写cli-queue-standards-review.md。明确违反和维护判断分开。气味基线仅heuristic，repo优先，跳过lint强制项：名称不明改名；重复共享；FeatureEnvy移动到拥有者；DataClumps/PrimitiveObsession领域类型；RepeatedSwitch统一映射；ShotgunSurgery聚合；DivergentChange拆模块；SpeculativeGenerality删除无需求抽象；MessageChains隐藏导航；MiddleMan删除纯转发；RefusedBequest用组合。引用file/symbol及规则，不凑发现。

方案轴写cli-queue-spec-review.md。重点检查所有阶段取消、不启动下一req；准备期cancelled130 vs可能写入unknown6；已确认写成功不能变unknown；resend无重试且保留parent404fallback、确认polling；delete两步、404幂等、unknown结果；状态优先/业务/协议/脱敏、cookie认证、URL参数、共享helper；fixtures每read绝对deadline，drain/join/child回收；实际Red不是setup失败。明确未验证边界，别报其他blocking/桌面不在范围作为缺陷。父负责集成、测试和每项处置。
