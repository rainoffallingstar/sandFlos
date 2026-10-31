# 前置取消切片 Standards 审查

范围：静态审阅最终工作区中新 `cancellable_prepare.rs`、`pre_write_cancel.rs`、TDD 文档及 `api.rs`/`services.rs` 切片；未运行测试、未改产品。撰写时父审查修正尚在收尾。

确认问题（维护判断）：

- `crates/core/src/services.rs:1509-1517,1537-1545,1570-1578` 三次重复 `RequestError→ServiceError` 映射；同文件 `:1371-1384` 已有 `classify_request`。属 Duplicated Code，应提取共享映射并保留后写入语义差异。
- `crates/cli/tests/pre_write_cancel.rs:40-69,83` 每连接 worker 存入 `workers`。若 fixture `Drop` 仍未实际 join（现实现仍在修正），则违反 AGENTS.md owned lifetime，且 `:420-429` 无后续请求断言有竞态；须释放 held 响应、drain 并 join。
- 测试用固定 `:26-29` 界限、纯回环，符合 synthetic/bounded；`cancellable_prepare.rs:126` 重试前查取消，无取消后新请求。

通过：新逻辑位于 core 且不依赖 UI，符合 README 项目结构；URL/响应解析复用 `api.rs:9-11` helper；TDD 文档 `:84-96` 如实记录剩余限制。

未发现其他确认缺陷；未审运行行为与签名正确性。

## 父集成处置

- 生命周期项已修复：最终 `pre_write_cancel.rs:145` 的 finish 先停止/释放并 drain accept 队列，实际 join listener 与所有 worker 后返回记录；Drop 也 join。`:268`/`:315` 由父集成改为显式逐次 read，每次重查同一个绝对截止时间；不会依赖 read_line/read_exact 内部的相对超时。最终 7 项公共用例通过，日志 `/tmp/prewrite-parent-integration.log`。
- 错误映射重复属于维护判断。三个后写入方法沿用本切片前已审实现，本轮不扩大范围改动其确认成功/unknown 行为；四个新准备入口共用 `classify_request`。保留该维护债务，不当作行为缺陷。

本轴共 2 项：1 项生命周期缺口已关闭，1 项已有映射重复的维护判断保留。
