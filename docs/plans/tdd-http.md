# CLI HTTP 与进程验收记录

日期：2026-10-29。继续实施 `cli-review-and-tdd.md` 的 S1–S4 公共边界；子 agent 使用 DeepSeek V4.1 Flash、high，协调 agent 负责接口、书库修复和最终审查。

## 协议边界

生产客户端使用固定官方 API 和由 bucket 生成的 OSS 地址。核心层的 `ApiEndpoints` 显式传入适配器，不读取进程环境变量。只有 CLI 入口读取 `SANDFLOS_TEST_API_BASE_URL` 和 `SANDFLOS_TEST_OSS_BASE_URL`；仅 API 地址时 OSS 默认同源，孤立 OSS 或无效地址退出 2。认证测试地址采用同一限制：只接受字面回环 IP 的 HTTP(S) origin，拒绝凭据、路径、query、fragment 和非 Unicode 输入；无效设置不能静默回退生产服务。

测试运行真实 CLI 子进程、真实 HTTP 适配器与回环 TCP 服务，检查对外请求和 JSON/退出码。夹具使用合成账号、cookie、STS 与协议数据，不是脱敏的生产流量采集；通过这些测试不能证明所有生产业务码和设备行为。

## Red/Green 与回归的区别

- `invalid_reading_dates_are_usage_errors_before_credentials` 首次返回退出 3 / `missing_credentials`（预期退出 2）；将日期验证放到凭据解析前，随后通过。
- `invalid_auth_origins_are_rejected_without_configuration_changes` 首次对带路径的测试认证地址返回退出 0（预期退出 2）；统一校验 origin 后通过。
- `ambiguous_resend_submits_once_and_reports_unknown` 首次得到 `(4, Network)`（预期 `(1, Unknown)`）；取消重推写入重试，保留提交前/后错误阶段，提交结果不明退出 6。
- `unavailable_quota_stops_before_oss_upload` 首次记录到配额 500 后继续请求 bucket、STS 和 OSS；上传身份上下文现在传播配额错误，只读查询和队列改用独立身份上下文。
- `unconfirmed_message_write_is_unknown_and_keeps_the_resource_key` 首次得到退出 4、unknown=0（预期退出 6、unknown=1）；写入网络/协议错误保留未知结果及 resource key。
- `resend_parent_fallback_preserves_the_original_format` 首次把 EPUB 重建为 PDF；从原消息内容保留格式后修复。
- `queue_http_requests_honour_the_configured_timeout` 首次在配置 1 秒超时后仍接受 2 秒慢响应；neocloud 改为复用调用方客户端，按请求 URL 选取同步 cookie，从而继承超时和重定向策略。
- WebDAV 附件与含空格密码的实际 Red/Green 见 `cli-libraries-execution.md`。
- 已存在业务实现的 happy path、错误脱敏与打包验收属于新增回归保障，不笼统宣称每项都有先 Red 后 Green。并行开发引入的编译错误和 lint 告警不算业务 Red。

## 审查要求

- 推送结果表示云端接受；同步未确认、最终提交失败或响应不可解析必须保留未知状态，不能退出 0。
- 未证明提交幂等时，502、超时或响应丢失不能触发盲目重提；客户端不能假定服务端没有执行。
- 队列删除保留两步结果，404 按幂等语义处理；凭据存储损坏保持本地 I/O 退出 5。
- HTTP 失败分类和诊断不能输出凭据、原始响应正文或签名 URL。

最终测试数量、release、CI 和剩余人工验收以 `tdd-integration.md`、`cli-handoff.md` 为准。

2026-10-30 跨平台回归收尾：Windows 认证/查询和 macOS 重推曾在不同轮次提前返回网络或协议错误。回环 fixture 的非阻塞 listener 接受连接后，现显式将连接切回阻塞模式并设置读写超时；WebDAV fixture 按行读完请求头后再响应。未放宽业务断言，也未改变生产请求重试策略。最终 `25aeab2` 的三平台检查和下载包运行在 run `40976698508` 全部通过；此项属于夹具修复证据。

Calibre/Zotero 进程回归使用真实临时 SQLite 和不同内容的附件，从 configure 到 push 验证实际 OSS 请求体等于所选文件，并逐字节确认源数据库未改变。该功能此前已实现，此处属于新增端到端回归。

后续 `49117e6` 补齐真实 CLI 的 WebDAV 下载→统一推送组合回归，覆盖成功与 `saveAndPush` 未知结果；除上传字节与源库不变外，直接检查公开报告 path 指向的文件及其临时父目录已清理，不以一个未被使用的空目录作为清理证据。目标测试 4 项通过，最终 Linux 统一检查为 158 项 Rust 测试和 5 项 Python 测试；run `40978370161` 的三平台检查、下载包运行及桌面编译共 7 项全部通过。
