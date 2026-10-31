# 进度/TDD/技术债计划审计

用户再次要求check progress/TDD/unfinished/技术债/GitHub，制定下一步计划，可必要破坏性重构。父已核对GH：最新41024736353是旧c460ab6，远端cli仍3cc6fb7，当前未提交无CI。禁止commit/push/真实账户/tag/release仍适用。

读cli-handoff顶部、cli-state-migration、cli-plan-status、cli-progress-tdd-audit及必要源码。最多6次read/tool调用，随后写docs/plans/cli-next-steps-and-debt.md，不运行测试不编辑其它文件。区分：已完成自动化（221Rust7Python历史final，不重跑冒新工作）、进行中query取消（另agent负责，不声称完成）、明确未完成、人工/权限外部验收、纯维护判断。TDD指出真实test-first/直接Green/历史Red仅文档或临时日志的限制，不补造。

计划要可执行有先后与完成条件：本轮query取消优先；之后queue读取/修改取消保持unknown不盲重试；桌面app-owned状态迁移（login/Read接口已具备，凭据显式迁移、dashboard错误不能空列表吞掉、upload互斥owned、后续旧全局退出）；最后当前SHA三平台CI及人工平台验收。核对当前app调用链适当引用文件，避免只复述历史行号。给技术债优先级和合理的破坏性重构界限，明确现阶段不需要破坏兼容/删除用户缓存。不提确认问题；父有持续授权。
