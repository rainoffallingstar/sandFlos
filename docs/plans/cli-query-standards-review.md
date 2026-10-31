# 查询切片 Standards 审查

审查时源码为未提交工作区，固定基线 `3cc6fb7f29167bd722487c5fde0008c29f3f63ae`。未发现明确规范违反：查询业务留在 core，CLI 仅绑定取消标志并渲染，符合 README 的 core/UI 分工；URL、参数、身份解析和 `envelope_json` 与既有路径共享，测试使用 loopback 与合成 token，未见真实凭据、提交或推送。

可维护性判断（非规范违反）：

1. `cancellable_query.rs:35-42,51-57,66-72,82-114` 在 identity、devices、storage、reading 中重复“发送请求→解析信封→检查取消”的序列。后续新增查询容易继续复制；可提取一个只负责 GET + 信封解析的小辅助函数（重复逻辑→共享）。
2. `identity_request` 的 credential trim 与空值校验与既有认证入口存在重复逻辑；若 legacy 已有同一校验，应共享，避免认证错误文案和状态分类分叉。
3. `cancellable_query.rs:117-123` 的 `prepare` 名称过于宽泛，读代码时不易看出它同时创建 runtime 和 client；建议改为能表达其职责的名称（名称不明→改名）。

限制：仅静态审查，未运行测试，未检查完整 fixture 实现。

父agent处置：前三项保留为维护判断。本次仅有固定数量查询，现有共享URL/参数/信封解析已保护协议，不为这些转发再引入通用框架；后续统一只读传输时可合并。审查时构造器确实创建后丢弃blocking client，这是本次改动引入，并非基线旧问题；父agent已移除此无用构造。最终client创建错误仍由实际async调用按类型返回。runtime/client每次公共调用自有，读取三阶段共用同一对资源；资源复用优化不在本次功能验收内。
