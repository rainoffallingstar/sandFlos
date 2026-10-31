# 查询取消切片的集成审查

日期2026-10-31，固定基线 `3cc6fb7f29167bd722487c5fde0008c29f3f63ae`，含工作区与未跟踪文件；基线到HEAD提交列表为空，修改尚未提交。`git diff 3cc6fb7 -- crates/core/src/api.rs crates/core/src/services.rs crates/core/src/lib.rs crates/cli/src/query.rs`，并直接读取未跟踪 `crates/core/src/cancellable_query.rs`、`crates/cli/tests/query_cancel.rs`。前四片已通过221Rust/7Python完整检查，本次只审查新增query切片，不重新报告已处置的旧切片。可用 `/tmp/sandflos-query-review-base/{api.rs,services.rs,lib.rs,cancellable_prepare.rs,cancellable_push.rs}` 对比本轮开始前的源码，避免误归因。

方案源：`cli-query-cancel-brief.md`；上层方案 `cli-review-and-tdd.md` 的S1-S4与T9；实施证据 `tdd-query-cancel.md`。规范源：AGENTS.md的共同限制、上述brief、README中业务逻辑放core/UI放app的约定。禁止提交/推送/真实账号。只做静态审查，不运行测试或修改产品代码；总计最多6读/工具调用，报告<=400字，写指定文件后结束。

规范轴写 `cli-query-standards-review.md`：指出明确规范违反和可维护性判断，分开陈述，给文件/符号/规则；跳过工具强制项。气味基线仅为判断：名称不明→改名；重复逻辑→共享；Feature Envy→移动到拥有者；一组字段反复传递→类型；primitive obsession→领域类型；反复switch→同一映射；shotgun surgery→聚合同一改动；divergent change→拆模块；speculative generality→删除无需求抽象；message chains→隐藏导航；middle man→删除纯转发；refused bequest→组合。repo明确约定优先。不为凑发现报缺陷，旧baseline已有问题注明。

方案轴写 `cli-query-spec-review.md`：逐项对照响应头/body stall取消、auth到query和reading连续请求间检查、不丢既有成功/不启动后续、status-first/业务分类/脱敏、身份不附带storage、日期query/null/timezone、endpoint兼容与真实Red。检查fixture有界read、真正join/child回收、受控竞态是否实际证实取消已观察。发现带具体file/line和brief条款；无确认缺陷也说明限制。

父agent负责确认每项发现、处置和完整check_cli。报告反映审查时源码，不能把旧CI或历史Green当本次证据。
