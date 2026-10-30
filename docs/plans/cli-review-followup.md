# CLI 集成审查收尾

2026-10-30。固定点 `4d6a48a139851c7f169ed83bd0e1055031d86587`，审查覆盖本轮提交与未提交实现。DeepSeek V4.1 Flash（high）分别审查规范和方案，父 agent 复核并修复。该文不把审查时的旧测试数量当作最终验收；最终代码、数量和 CI run 见 `cli-handoff.md`。

## Standards

依据 README 的 core/CLI 分层约定、已确认的 S1–S4 和有限等待要求；格式/lint 最终由工具门禁负责。

| 发现 | 处理 |
| --- | --- |
| 新测试尚未统一 rustfmt，旧文档“已通过”不代表当前工作区 | 所有写入结束后统一格式；交班记录只引用最终统一检查 |
| SIGINT 夹具使用 512 KiB、无子进程 watchdog、慢读循环无总截止时间 | 改为 8 MiB 稀疏文件、停止位/截止时间、kill/reap watchdog；真实提交中断也有用例 |
| 取消可能丢失消息 ID/rev，并覆盖已收到的成功确认 | 真实 HTTP Red/Green 保留已知身份、未确认结果 unknown、已确认提交 accepted；下一阶段仍检查取消 |
| 取消测试 UUID 临时目录缺清理（工程判断） | TempTree RAII；失败断言也释放目录 |
| ResendError 的隐式 String 转换抹平错误含义（类型建模判断） | 删除 blanket 转换；三个旧字符串边界显式转安全 Protocol，HTTP 错误保持类型 |
| reader 用 Interrupted 可能被重试（可靠性判断） | 非重试错误 Other；真实大于缓冲区的文件取消验收 |

本轴 6 项发现；最严重的取消身份/确认丢失已修复。

## Spec

依据 `cli-review-and-tdd.md` 的 R4/R5 和 T7–T13 契约。

| 发现 | 处理 |
| --- | --- |
| 合法 JSON 缺 `results` 数组被当作健康空队列，违反“空队列 vs 查询失败” | Red/Green：必须是数组；空数组成功，缺失/null/对象失败为 Protocol |
| Calibre 只验证 books.id；空库测试无法证明拒绝同名无关表 | Red/Green：真实 books(id) 碰撞用例；校验列表/格式选择所需 books/data 列，源库不变 |
| 最终提交确认与取消的描述和真实适配器不一致 | 明确保留已收到的成功确认；响应未确认才 unknown；修复适配器并同步文档 |
| 本地预检先于取消，可能继续处理整个批次 | Red/Green：已有取消信号时缺文件也返回 cancelled/130；core 和 CLI 的预检均检查同一取消信号 |
| 迁移先拒绝已有目标，源/目标同时有问题时错误依赖目标状态 | 保留目标优先保护；原方案没有要求源优先。明确这是诊断顺序选择，不改变源或目标 |
| 手动 release 输入叫 tag，实际校验所选 ref 的 manifest | 输入说明和使用文档明确：手动打包所选 ref，tag 字符串是预期版本，不切换同名 tag；真实 tag 事件另行验收 |
| 两文件取消未断言第二项路径 | 补第二项 path 与 display_name，防止仅凭数组位置通过 |
| WebDAV 部分 fixture 仍阻塞 accept | 五处复用有限 accept/读写设置，并限制 join；不再以 join 上限掩盖阻塞 accept |

本轴 8 项发现；最严重的空队列误判已修复。生产协议、平台凭据、扫码、设备接收和大文件性能仍按逐项状态文档单列验收。
