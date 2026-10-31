# 前置 HTTP 取消集成审查

实现完成后执行；仅审查此切片。基点仍为 `3cc6fb7f29167bd722487c5fde0008c29f3f63ae`，工作区含此前已审三个切片。本轮增量可用 `/tmp/sandflos-prewrite-review-base/{api,services,push,lib,cancellable_push}.rs` 与最终源码比较；另完整阅读新 `cancellable_prepare.rs`、`pre_write_cancel.rs`、`tdd-prewrite-cancel.md`。`git diff 3cc6fb7 -- crates/core/src crates/cli/tests/pre_write_cancel.rs`不含未跟踪新文件，须直接读新文件。基点到HEAD无新commit。

Spec来源：`cli-prewrite-cancel-brief.md`（含父中间审查修正）、`cli-review-and-tdd.md` T7/S1/S3、AGENTS.md总约束。其余登录/Read/后写入切片已经审查，勿重复历史误报。尤其原签名canonical resource目前存在；不要凭截断输出判断遗漏。

Standards来源：README.md:89 新业务逻辑优先core且独立于UI；AGENTS.md实际行为Red/Green、synthetic/bounded fixtures、owned lifetime、禁止真实账号/commit/push。代码约束由fmt/clippy执行者不重复列为发现。

Standards smell baseline（全是判断项，非硬违规；repo明确约定优先）：
- Mysterious Name：名称不揭示目的，考虑命名或设计。
- Duplicated Code：相同逻辑重复，考虑提取共享。
- Feature Envy：主要访问另一对象数据，考虑移动方法。
- Data Clumps：参数组反复同行，考虑一个类型。
- Primitive Obsession：领域概念只用原始类型，考虑领域类型。
- Repeated Switches：同一类型的分支重复，考虑共享映射/多态。
- Shotgun Surgery：一个变更散落多文件，考虑聚合职责。
- Divergent Change：一个文件因多个无关原因变化，考虑拆分。
- Speculative Generality：没有实际需求的抽象，考虑删除。
- Message Chains：调用方依赖长链导航，考虑封装。
- Middle Man：纯转发中间层，考虑直接调用。
- Refused Bequest：继承者忽略大部分接口，考虑组合。

每个审查agent最多5次读/工具调用后写<=400字独立报告。不要运行测试、编辑产品或提交，不扩张穷尽检查。发现必须引具体最终文件行和spec/standard规则，并区分确认缺陷与维护判断；无确认缺陷时明说审查范围/限制。报告分别 `cli-prewrite-standards-review.md` / `cli-prewrite-spec-review.md`。父负责处理发现及运行统一检查。
