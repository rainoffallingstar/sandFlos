# 本轮双轴审查任务

固定基线：3cc6fb7f29167bd722487c5fde0008c29f3f63ae（HEAD仍在此；所有本轮变更在工作区，包含未跟踪文件）。使用 `git diff 3cc6fb7`，另读 `git ls-files --others --exclude-standard` 中的新生产模块/测试/证据。不要用 three-dot HEAD 比较漏掉工作区。

规范来源：根 AGENTS.md；docs/plans/cli-review-and-tdd.md 的 S1–S4、真实行为 Red、合成回环、有限等待、不泄密、旧兼容入口与不盲重试要求。没有单独编码规范。跳过格式/lint已强制项目，父运行工具检查。

方案来源：docs/plans/cli-state-migration.md 第二阶段的登录交付与进度接口，以及根 AGENTS.md 三个原始任务和 review corrections。本轮只补三个接口切片，不做桌面迁移；不要将计划中其余桌面工作标为本轮漏项。

Standards reviewer：只读审查，写 docs/plans/cli-round-standards-review.md，400词内发现+文件行号，区分硬规则违例与判断项。审查所有变更及新增文件。基线启发式（仅判断，不是硬规则；仓库规范优先）：Mysterious Name（名字不揭示意图，重命名）；Duplicated Code（重复逻辑应共享）；Feature Envy（多用其他对象数据，移到数据处）；Data Clumps（总一起传的参数应成类型）；Primitive Obsession（字符串替代领域概念）；Repeated Switches（重复分支可集中）；Shotgun Surgery（一个变化散落多文件）；Divergent Change（模块承担无关变化）；Speculative Generality（无需求的抽象/参数应删）；Message Chains（过长导航需封装）；Middle Man（仅转发无价值层）；Refused Bequest（继承后忽略大部接口）。不要把工具会发现的问题列为人工发现。

Spec reviewer：只读审查，写 docs/plans/cli-round-spec-review.md，400词内。找缺失/部分需求、越界、实现错误，每项引方案/AGENTS具体规则及实现行号。关注真正join/不能detach、取消后新请求、确认成功保留、unknown身份、读取与确认区别、默认trait兼容、错误状态优先与脱敏、测试是否真实公开边界。不要改生产/测试。

两位审查者无需跑测试，父正在统一验证。输出即使无发现也须写文件，清楚说明审查范围。不要commit/push。

## 续接审查（2026-10-30）

所有writer已完成，父统一验证214项Rust/7项Python、fmt/clippy/release/package全部通过，本次勿重复执行。当前Spec文件是父补验，请保留它，在末尾追加独立审查结果（含实际文件行号与需求引用），不要覆盖父补验。完成范围只包含独立Spec静态复核；不扩桌面迁移或commit/push。不跑web、编译、测试，不遍历无关历史；集中读最终三个生产切片与对应新增公共测试，完成后立即写结论。即使有未覆盖风险也可在报告中明确列出，勿因追求穷尽而无限延长。

最终实际源码中的OSS签名和独立HMAC期望均有canonical resource（包含转义的换行）；之前Standards报告对此误读，父处置有实际源码依据。仍须自行读取最终实现，勿照抄任一报告的结论。同步cookie/状态优先/取消后的新请求/确认结果保留/portal真正join/Read与Uploaded契约是重点。只修改指定审查文档；完成后简报发现，不宣称运行过任何测试。
