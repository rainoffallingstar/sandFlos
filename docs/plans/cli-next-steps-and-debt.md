# CLI 下一步与遗留技术债

日期：2026-10-31。基线 `3cc6fb7f29167bd722487c5fde0008c29f3f63ae`，全部续实施改动仍在工作区。遵守 AGENTS：不提交、推送、tag/release，不使用真实账号、凭据或书库。实施使用 DeepSeek V4.1 Flash（high），父 agent 串联、审查及集成。

## 当前进度

此前六片已完成：浏览器登录显式令牌交付与 owned portal、连续读取进度、后写入取消、push 前置 HTTP 取消、只读查询取消、queue 取消及 unknown 语义。此前统一检查255 Rust、7 Python全通过，日志 `/tmp/sandflos-cli-queue-final-check.log`，不重复实施。

本片完成桌面刷新第一步：`DashboardRefreshInput` 接收显式凭据、上传投影、本地日期和互传投影；严格入口返回类型化错误，不把 queue/统计/device 请求失败变成健康空数据。app 实际使用 Result 入口，失败保留旧快照及同步时间并显示原因，成功更新 app 快照与托盘，不再写 core dashboard cache。首次未取得快照的队列视图不显示“队列为空”。注入端点路径不进行生产 WebSocket 或真实 LAN 探测，生产 wrapper 保留这些能力；旧兼容 API 仍在。

本片新增12项公共 HTTP 回归。双轴审查无确认硬规范/方案缺陷；全局cache测试恢复维护发现已修复；仅测试修订后父重跑12项公共回归、core严格clippy与格式检查通过，日志 `/tmp/sandflos-desktop-refresh-targeted.log`。父最终统一检查已通过：267 Rust、7 Python，fmt/strict clippy/release/依赖边界/无显示/打包运行通过，日志 `/tmp/sandflos-cli-desktop-refresh-final-check.log`。GPUI 离线检查缺 `aes v0.8.4` 缓存，未进入 app 类型检查，不能称桌面编译通过。

## 未完成任务与执行顺序

1. **app-owned 登录与持久化。** 桌面仍在初始化时 `hydrate_auth_state`，登录走旧 `start_login_flow`，刷新暂从 `get_auth_state` 取 token。下一片由 app 持有 `LoginPortal` 和会话；回调经显式 `AuthService`/路径持久化，失败不更新会话、不报成功，成功只交付一次；明确目录与凭据后端，不静默明文降级，也不自动改写/删除旧 `auth_cache.json`。保留旧公共包装器。
2. **app-owned 上传及业务调用。** 刷新仍从 `current_upload_snapshot` 取上传投影；文件选择、Calibre/Zotero 推送、重推/删除仍走旧全局方便函数。由 app 会话持有单任务互斥和进度，统一调用 `PushService`/`QueueService`；先共享书库解析返回路径/待上传项，再替换最终上传，不复制 SQLite/WebDAV 协议。Read 只表示本地读取，Uploaded 只表示 OSS 确认；unknown/已知消息身份/部分成功不能被“上传成功”通知覆盖。
3. **桌面集成验收。** 在可用 GPUI 平台完成类型检查与窗口/托盘/登录/上传人工回归。当前 Linux 依赖阻塞时不能用 core 测试代替。用 `rg` 核对 app 产品路径不再调用全局 auth/cache/upload helper；并发上传由 app 拒绝；刷新失败、登录保存失败和 unknown 有可见结果。全局兼容入口至少保留到迁移及人工回归完成。
4. **剩余取消边界。** 登录已开始的上游请求仍可等待20秒；应用交付 callback 必须有界返回。旧桌面 blocking 路径尚在；DNS/连接建立的系统等待未单独验证。生产设备发现 WebSocket 建连及本地探测也不在本片取消保证内。
5. **当前代码远端与真实平台验收。** GitHub 最新成功仍为旧 `c460ab6`/run `41024736353`（7/7），远端 HEAD `3cc6fb7`。当前未提交代码没有对应 SHA/run。AGENTS 禁止 commit/push，因此本轮只读核对；不得把旧绿色当当前验收。真实 Keychain/Credential Manager/ACL、扫码、BOOX/OSS/neocloud/WebDAV、设备接收、统计时区及 tag 到 draft release 仍未验收；accepted 只表示云端接受。

## TDD 审计

S1–S4 公共边界已确认。查询、queue 和前置auth取消保留真实行为 Red/Green；共享机制用例与审查追加回归记直接 Green，编译/setup/fixture 问题不算产品 Red。本片首位 writer 报告“提取后 queue503 返回成功空快照”的 Red，但父没有取得原始运行日志；只记为报告的历史，最终12项 Green 不能反推 test-first。详见 `tdd-desktop-refresh.md`。禁止回滚实现补造 Red；后续新片继续先建立可运行的公共失败行为。

## 维护债与重构边界

可按需要重构 app 状态容器、注入凭据/缓存路径、收口业务服务调用并删除 app 对全局 helper 的依赖。保留 CLI/HTTP 公共兼容与用户数据，不为“清零技术债”删除旧凭据或书库。取消传输及错误映射仍有少量重复，等业务迁移稳定后收口；文档历史行号以当前符号为准。actions Node20/24 维护提示不等于构建失败，依赖升级单列并核对官方迁移说明后再做三平台产物验收。

整体清单仍未全部完成。本片的刷新错误保留已实现；完整桌面会话、上传迁移和平台验收继续单列，不能合并标绿。
