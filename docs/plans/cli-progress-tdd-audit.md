# CLI 进度与 TDD 核对

日期：2026-10-31。只审文档与最终 Green 日志，未改代码、未重跑测试，未用真实账号、凭据或书库。

- 登录交付：Red 是公开 API 行为失败：回调失败仍返回 `ok:true`，open 失败后监听未停。审查补验记录真正 join 的两个行为 Red：部分请求后 drop 仍触发回调，旧登录替换未关闭前一 portal。日志中 `login_delivery` 为 6/6 Green。
- 后写入取消：原始三个子进程用例被 8 秒 watchdog 判失败，夹具已收到目标请求，属于阻塞等待的行为 Red。审查补验记录 malformed 确认首次退出 6/unknown，以及 syncToken 悬停时仍发出消息 PUT；Green 为 `post_write_cancel` 5/5。文档如实说明未收到的 PUT 响应不产生客户端已知 ID/rev。
- 读取进度：旧 CLI 在 1 MiB 已读完且 OSS 响应被压住时无读取样本，属回归 Red。审查补验覆盖全样本单调与上限、16 MiB 慢读多样本、独立 HMAC Authorization（修复 canonical resource）、取消后抑制最终样本；Green 为 `transfer_streaming` 6/6 与 CLI `read_progress` 1/1。

三份记录均未把编译、缺少 API 或夹具启动失败算作 Red。限制：`/tmp/sandflos-cli-round-check.log` 只证明最终 Green，不能独立重建历史 Red；Red 由文档记录佐证。测试均为回环合成，未证明真实平台行为、桌面迁移或人工验收。历史已完成测试未重跑冒充新实施。`cli-plan-status.md` 已按当前工作区基线更正。

## 同日后续：父集成新增切片核对

上述是此前三个切片的只读审计。随后按用户要求，由DeepSeek V4.1 Flash（high）实施并专项修订push前置HTTP取消，父agent集成审查，记录见 `tdd-prewrite-cancel.md`。

父实际读取了 `/tmp/prewrite-red-auth.log`：首个auth公共CLI用例在目标请求已到达后，SIGINT仍触发8秒watchdog，属于真实行为Red；实现后Green。其余phase测试在共享机制实现后补充，按直接Green回归记录；实现后基线五失败对照不冒称逐项test-first；中间信封解析错误也未计作慢storage取消Red。最终7项中6项为CLI SIGINT、1项为公开backend受控取消标志/成功响应竞态，后者替换50ms sleep版本。

最终Green由父运行完整 `CARGO_NET_OFFLINE=true bash scripts/check_cli.sh`：221项Rust、7项Python，0失败；fmt、严格clippy、release、依赖边界、无显示及打包运行均通过，日志 `/tmp/sandflos-cli-prewrite-final-check.log`。这是新增代码后的验收，不冒用此前214项结果。GitHub最新run仍只覆盖旧 `c460ab6`，当前工作区未提交/推送，远端CI仍待完成。
