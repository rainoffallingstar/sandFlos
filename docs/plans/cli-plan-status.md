# CLI 方案逐项状态

更新：2026-10-31。基线仍 `3cc6fb7f29167bd722487c5fde0008c29f3f63ae`。此前六片完成；本片新增桌面显式刷新及错误保留，12项公共回归，父完整267项Rust、7项Python与fmt/strictclippy/release/产物检查全通过。改动未提交推送，没有当前代码三平台CI；旧 `c460ab6`/run `41024736353` 7/7只对应旧SHA。GPUI离线检查缺aes缓存，未进入app类型检查；完整桌面登录持久化/上传会话与人工验收仍未完成。原方案 `cli-review-and-tdd.md`；最新验收 `cli-handoff.md`，技术债及下一步 `cli-next-steps-and-debt.md`。本片Red仅有writer报告、父无原始日志，不能列为独立核实test-first，见 `tdd-desktop-refresh.md`。

| 切片 | 已实现或补验 | 尚需单列的验收 |
| --- | --- | --- |
| T0 | 固定 Rust 1.97.1；Cargo.lock；无显示检查和依赖闭包检查；旧 SHA 三平台 CI 已通过 | 当前变更未提交、未推送，尚无当前变更的三平台 CI；旧 `c460ab6` 的 run `41024736353` 7/7 只证明旧 SHA |
| T1 | 独立 CLI crate、帮助/版本、参数错误、Unicode 路径与 JSON 信封 | 无新增功能缺口 |
| T2 | 配置隔离、优先级、文件锁、原子替换、损坏存储处理；旧值跨进程仍可用 | 真实平台权限与 Windows ACL；磁盘故障没有穷举 |
| T3 | stdin 导入、校验、持久化、环境覆盖、注销、存储/删除失败、输出脱敏 | 平台 Keychain/Credential Manager 的实际会话环境 |
| T4 | QR pending/confirmed/expired/rejected、缺 token、等待/HTTP 超时、取消、网络失败；非交互拒绝；新增应用侧 token 交付回调和单一路径、失败不报成功、重复成功不二次交付、owned portal 真正 join 与有界请求 IO，legacy 包装器保留私有 portal 持有 | 手机扫描终端二维码与真实登录；真实 WeChat/captcha/平台凭据人工验收 |
| T5 | 真实适配器的 OSS、消息写入、同步、单次最终提交；保留消息 ID 和 resource key | 生产 BOOX/OSS/neocloud；云端接受与设备收到分开记录 |
| T6 | 逐文件成功/失败/unknown、稳定聚合退出码、输入/200 MiB/配额检查 | 真实大文件性能属于 T7 人工验收 |
| T7 | 有长度的流式 body；OSS 写背压下可取消异步 PUT；慢连接/中断/确认丢失；STS 到期；阶段/批次取消；新增连续 `Read` 事件明确区别于 `Uploaded`（OSS 确认），本地读取完成不表示远端接受；新增 `write_message`、`confirm_sync`、`save_and_push` 的后写入 SIGINT 取消并保留已收到身份、不盲重试；push认证/配额/bucket/STS/签名链接验证取消、body悬停与受控响应竞态均通过；Linux 200 MiB 字节/哈希与采样内存、吞吐实测 | 生产慢链路和其他平台性能；采样不等于精确最终峰值。query的身份/设备/空间/阅读请求取消已补齐，登录上游及旧桌面路径仍不在本次取消范围；DNS/连接建立的系统等待未单独验证。见 `cli-slow-cancellation.md`、`cli-upload-performance.md`、`tdd-read-progress.md`、`tdd-postwrite-cancel.md`、`tdd-prewrite-cancel.md` |
| T8 | 有限队列窗口、查询错误保留、显式删除、404、两步部分完成、重推一次和 unknown；准备阶段保留认证错误类别 | queue读取/修改取消已补齐，23项公共测试通过，写前130/未知6，确认成功与404幂等保留；生产删除职责与业务码语义尚需脱敏样本确认 |
| T9 | 独立查询、设备地址校验、合法日期、跨年与不同本机时区、缺数据保持 null；新增身份/各查询响应头及body悬停SIGINT取消，顺序请求间取消检查，11项公共用例通过 | 上游统计时区尚未确认，输出 `timezone:null`，不宣称 UTC/本地时间 |
| T10 | 显式多库、按库+格式 ID 精确选择、真实只读 SQLite、分页/搜索与失败分类 | 书库边界补验结果见 `tdd-boundaries.md` |
| T11 | 显式 Zotero 目录、本地优先、WebDAV 下载后统一推送、401/404、ZIP 限制与清理 | 真实 WebDAV 服务兼容性；边界补验结果见 `tdd-boundaries.md` |
| T12 | 显式迁移、拒绝覆盖、损坏/缺 token、重试、源文件不变、诊断脱敏 | 迁移后新进程校验结果见 `tdd-boundaries.md` |
| T13 | 历史三平台 CLI/下载产物检查、macOS 桌面编译、版本化 ZIP/校验和与独立 CLI tag workflow 曾验证 | 真实 tag 触发到 draft release 的事件链尚未执行；平台凭据、扫码、桌面人工回归仍需验收；不扩本轮范围 |

测试记录中直接 Green 的已有行为回归不补造 Red 历史；并行编译冲突、依赖下载错误也不算 TDD Red。历史已完成测试不重跑冒充新实施。各切片的历史记录与最终通过数可能不同，以交班记录所指的提交和 run 为准；当前工作区尚无对应 run。

架构迁移仍有明确债务：登录和读取进度已提供可供应用消费的公共边界，桌面刷新现已接入显式输入/Result，由app保留快照及错误、不写core cache；credential/upload仍是旧global过渡桥，完整app-owned会话尚未完成，core全局兼容入口仍存在。桌面迁移与真实人工平台验收未完成。连续读取进度表示本地已读字节，`Uploaded` 仍只在 OSS 确认后出现；push已覆盖前置准备和后写入取消，devices/storage/reading已覆盖身份与查询取消；登录上游和旧桌面blocking路径仍可能等待请求超时。真实线上业务码、统计时区、设备行为和桌面 UI 不能由合成 HTTP 夹具推定。
