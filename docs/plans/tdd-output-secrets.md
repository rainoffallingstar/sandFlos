# 类型化错误集成的脱敏回归

2026-10-30，S1 真实 CLI 进程 + S3 回环服务。此切片针对本轮类型化错误迁移中出现的回归，不改写上一轮已通过的历史记录。

`CARGO_NET_OFFLINE=true cargo test --locked -p sandflos-cli --test http_output_secrets`：

1. 回环服务正常接收 OSS 上传、写入消息，随后同步查询在 `_rev` 中放入 `upstream-secret-signature=fixture`，让服务无法确认版本一致。
2. Red：CLI 虽正确退出 6 并保留 unknown 和消息 ID，却将恶意 revision 拼进 `data.files[0].error.message`。测试在 stdout 脱敏断言处失败，记录 `/tmp/sandflos-http-secrets-red.log`。
3. 最小修复：同步失败只生成固定协议消息，不拼接远端 revision；服务层也对 Protocol 错误统一使用固定脱敏文本，避免后续适配器把原始错误再次带回 JSON。
4. Green：1 项通过、0 失败，约 7.6 秒，记录 `/tmp/sandflos-http-secrets-green.log`。原来 unknown、消息 ID 和退出 6 语义保持；测试还确认不会继续 `saveAndPush`。

HTTP 类型、状态码和业务码仍由结构化字段承载，不以错误文本进行认证/网络分类。用于测试的 secret 为合成样本，未访问真实凭据或账号。
