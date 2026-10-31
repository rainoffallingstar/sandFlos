# CLI 本轮 Spec 审查

基线 `3cc6fb7f29167bd722487c5fde0008c29f3f63ae`；范围为最终工作区生产代码、新增公共测试和证据，需求源为根 AGENTS.md 三个原始任务及 review corrections、`cli-state-migration.md` 第二阶段接口。独立 DeepSeek 方案轴审查最初未及时返回，父 agent 中止等待并完成下方补验；续接后由 DeepSeek V4.1 Flash（high）完成独立静态复核，结果追加在文末，保留此前过程记录。

## 父 agent 逐项结论

- 登录交付：`auth.rs` 的 `TokenDelivery::deliver` 为所有成功登录路径提供统一门控；保存失败不置 delivered，成功后不再调用。新 startup 不写 global/cache。stop 在交付 gate 下置位，drop 真正 join；旧入口通过私有 holder 保持。公共回环覆盖失败重试、重复、无 token/非法 state、QR/email、半截请求后 drop 和 legacy 替换。符合任务；同步应用回调必须有界返回，上游已有请求最多等待其原有20秒超时，均已在API/证据中说明。
- 读取进度：`PushService` 实际分派到默认兼容 observer，`Read` 与 `Uploaded` 分开；文件body计数经当前线程采样，严格递增、有界。16 MiB慢读取多次样本、独立OSS签名和取消后不再回调均通过公共HTTP边界验证。未将本地读取冒称远端接收。
- 后写入取消：owned runtime/future，无分离阻塞worker；请求前门控阻止取消后下一次请求，100ms收尾仅处理在途响应。已收到ID/rev与已确认成功保留，未确认写入/提交unknown且无重试。公共子进程覆盖三个慢响应、syncToken取消后无PUT、无效文档重试；旧blocking入口保持。

父审查曾发现并推动修复：OSS签名搬移遗漏、读取测试首样本假设、portal分离线程与mem::forget、无效文档提前退出、取消期间复合future继续发PUT。最终未发现未处置的范围内需求缺陷。桌面迁移、其他blocking HTTP取消、生产/平台人工验收是明确后续范围，不在本轮冒称完成。没有需求外的产品行为。

验证：父统一检查214项Rust、7项Python全部通过，fmt、严格clippy、release、依赖边界、无显示运行及产物打包校验通过；未执行本轮远端三平台CI。

## 独立 Spec 复核（追加，2026-10-30）

独立静态复核了最终工作区的 `crates/core/src/auth.rs`、`cancellable_push.rs`、`services.rs` 进度/上传/服务分派，以及 `crates/core/tests/login_delivery.rs`、`crates/cli/tests/read_progress.rs`、`crates/cli/tests/post_write_cancel.rs`、`transfer_streaming.rs` 的公共边界断言。未运行编译或测试。

未确认范围内 Spec 缺陷。登录 portal 在 `auth.rs:151-158` 真正 `join`，`TokenDelivery::deliver` 在 `auth.rs:196-207` 以 stop/delivered gate 保证停止后不交付、成功后不重复交付，回调时限和上游 20 秒边界已明示；legacy 由 `auth.rs:34-37,211-239` 的私有 holder 持有并替换。后写入阶段在 `cancellable_push.rs:182,186,208,216,241` 等请求前及轮询处检查取消，确认轮询在解析后重试无效文档（`:189-195`），不会把取消误报为远端成功。读取进度与 OSS ack 分离于 `services.rs:238-270,700-731`；最终签名串 `services.rs:1220-1224` 与独立 HMAC 期望 `transfer_streaming.rs:404-406` 均含 canonical resource，源码原始字节已核对。

限制：应用回调必须自行在短时限内返回；登录上游已发请求仍受既有 20 秒超时约束；桌面迁移不在本轮。测试未重跑，结论仅限静态代码与测试设计复核。
