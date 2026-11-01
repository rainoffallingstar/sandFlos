# CLI 改造交班记录

> **2026-11-01 方向变更（已终止）**：`cli` 分支后续只做纯 CLI，不再接入桌面端（方案 A）。本文档所述桌面切片不再作为本分支待办；未提交的桌面登录会话工作区已撤回并归档在 `/tmp/sandflos-desktop-session-wip`。当前范围与验证见 `tdd-cli-only.md`。以下内容保留为历史记录。

## 2026-10-31 最新续实施：桌面刷新显式输入与错误保留

从已完成queue取消的状态继续，保留此前六片，不重复实施。DeepSeek V4.1 Flash（high）完成实现及审查修订，父负责串联、独立复核、双轴审查和本机集成。HEAD仍 `3cc6fb7f29167bd722487c5fde0008c29f3f63ae`，代码保留工作区，未commit/push/tag/release，未使用真实业务账号、凭据或书库。

本片为桌面状态迁移第一步：新 `DashboardRefreshInput` 显式提供token、上传/互传投影、本地日期；严格刷新返回类型化Result，queue/阅读/device请求失败不再变成成功空数据。app实际调用新入口，失败保留旧快照及同步时间并通知，成功更新app快照与托盘，不写core dashboard cache；首次未取得队列数据不会显示“队列为空”。端点注入路径缺share投影时零请求拒绝，纯device映射不触发生产WS或LAN探测；生产wrapper及旧兼容入口保留原能力，映射逻辑共享。

新增12项公共回环HTTP回归。**父完整 `CARGO_NET_OFFLINE=true bash scripts/check_cli.sh` 返回0：267项Rust、7项Python，0失败**；fmt、严格clippy、release、依赖边界、无显示启动、打包及产物运行全通过。日志 `/tmp/sandflos-cli-desktop-refresh-final-check.log`。统一检查后仅修测试全局cache原值恢复；随后父重跑12项公共回归（0.20秒）、core all-targets严格clippy及core/CLI fmt全部通过，日志 `/tmp/sandflos-desktop-refresh-targeted.log`；产品代码未再更改。未重跑200MiB性能。

TDD：首位writer报告曾观察提取后queue503仍返回authorized空快照的行为Red，但父未取得原始运行日志，因此标记“已报告、未独立核实”，不称父已验证test-first。最终Green不能重建Red，未回滚代码补造历史；后续共享机制/审查修订均直接Green，详见 `tdd-desktop-refresh.md`。双轴静态审查无确认硬规范或方案缺陷；规范轴的cache恢复维护发现已修复并定向复验，详见 `cli-desktop-refresh-standards-review.md`、`cli-desktop-refresh-spec-review.md`。

桌面 `cargo check --locked -p send2boox-desktop-gpui` 离线尝试返回101：缺缓存 `aes v0.8.4`，未进入app类型检查。日志 `/tmp/sandflos-desktop-refresh-app-check.log`。此前writer网络尝试亦报告配置registry不可达；不能声称当前桌面构建/macOS人工通过。

GitHub只读复核：cli远端HEAD仍3cc6fb7；最新run41024736353对应旧c460ab6、completed/success（历史7/7）。当前工作区无对应CI；AGENTS禁止commit/push，本轮不执行。**本片刷新实现与CLI本机验收完成，整体计划未全部完成**：app仍以旧global helper取得credential/upload投影，登录持久化、上传互斥/进度、queue修改和Calibre/Zotero最终上传尚需迁移；登录上游/旧桌面阻塞、DNS/连接建立、当前变更三平台CI与真实平台/生产/设备/tag链路验收保留。下一步及重构边界见 `cli-next-steps-and-debt.md`。下文均为历史。


## 2026-10-31 最新续实施：queue取消与unknown

继续使用DeepSeek V4.1 Flash（high）实施，父集成与双轴审查。基线仍3cc6fb7，工作区未提交/推送，无真实账号凭据书库/tag/release操作。此前五片不重做。

queue list/resend/delete及认证/准备阶段已使用owned async取消；复用原URL、信封、cookie、签名及同步helper。写前取消130；已发出写入但未确认返回unknown6，不盲重试；已确认成功保留。parent同步期间取消为unknown，明确PUT404后重置写入标记，fallback实际PUT时重新置位。删除第一步确认后取消保留已完成部分；404幂等及明确认证/业务错误类别保留。用户timeout不缩短，无detached blockingworker。

真实Red：_changes请求已收到后SIGINT仍触发8秒watchdog；最小Green约0.29秒。新增23项公共测试，其余共享机制/父审查修订为直接Green；夹具及中间构造错误不冒充产品Red。fixture有绝对read截止、写超时、drain/join和child回收。详见tdd-queue-cancel.md。父发现并关闭nested runtime、parent取消误归Input、明确404后fallback写入状态三项问题。规范轴无确认违反，3项维护判断留档；方案轴的TDD交付缺口已关闭，父分支复核补充记录见cli-queue-spec-review.md。

最终 `CARGO_NET_OFFLINE=true bash scripts/check_cli.sh` 返回0：**255项Rust、7项Python，0失败**，fmt、strictclippy、release、依赖边界、无显示启动、打包及产物运行通过。新增queue23项统一检查1.42秒通过。日志 `/tmp/sandflos-cli-queue-final-check.log`。未重跑200MiB性能或桌面人工验收。

GitHub只读复核仍是旧c460ab6/run41024736353成功（7/7），远端HEAD3cc6fb7；当前未提交代码没有对应CI。AGENTS禁止commit/push，本轮未执行。整体仍未完成：桌面app-owned状态迁移与dashboard错误保留、登录上游/旧桌面阻塞路径、DNS/连接建立取消单独验证、当前代码三平台CI与真实平台/生产/设备/tag链路验收。下一步先实施桌面迁移，保留公共兼容入口与用户缓存。详见cli-next-steps-and-debt.md。以下为历史记录。

## 2026-10-31 最新续实施：查询取消与技术债计划

用户再次要求核对进度、TDD、未完成项、技术债及GitHub，父agent按优先级补齐 `HttpQueryBackend` 的查询取消。实施继续使用 **DeepSeek V4.1 Flash（high）**，父agent串联、双轴审查和集成。基线仍为 `3cc6fb7f29167bd722487c5fde0008c29f3f63ae`，全部新代码在工作区；遵守AGENTS的禁止提交/推送约束，无tag/release、真实业务账号、凭据或书库操作。

- users/me身份查询、devices、storage、reading三个顺序请求使用现有owned async runner。请求前及响应后检查取消，取消后不发下一请求；身份查询仍不附带storage。只读查询可丢弃已完成数据返回Cancelled；push已确认写入成功/已知消息身份的契约保持。
- URL、日期参数与状态优先信封解析由新旧路径共享，保留业务/协议错误类别、脱敏、null和 `timezone:null`，CLI绑定同一SIGINT标志。用户timeout未缩短，无detached blocking worker。
- 新增11项公共测试：8项真实CLI SIGINT覆盖身份/各查询响应头及身份、storage信封body；1项公开backend预取消，2项先发布取消再释放响应的受控竞态。fixture逐次read检查绝对截止，drain排队连接并真正join线程、回收子进程。
- 实际Red为devices请求已到达后SIGINT仍触发8秒watchdog；同一用例最小Green约0.43秒。其它用例是共享机制的直接Green回归。父核对原始Red日志，未将编译/setup失败或最终Green反推为Red。见 `tdd-query-cancel.md`。

最终 `CARGO_NET_OFFLINE=true bash scripts/check_cli.sh` **全部通过：232项Rust、7项Python，0失败**；fmt、严格clippy、release、CLI依赖边界、无显示启动、打包及产物运行通过。11项查询测试在统一检查中0.36秒完成。日志 `/tmp/sandflos-cli-query-final-check.log`。没有重跑200MiB性能，也没有执行本轮桌面或真实平台验收。

双轴报告：`cli-query-standards-review.md`、`cli-query-spec-review.md`。规范轴无确认违反，3项维护判断已记录父处置；构造器无用blocking client已删除。方案轴无确认缺陷，父补充query端点body-stall回归并复核fixture生命周期。下一步与技术债优先级见 `cli-next-steps-and-debt.md`：queue取消/unknown → 桌面app-owned状态及dashboard错误保留 → 当前代码三平台CI和人工平台验收；允许必要内部重构，保留公共兼容和用户数据。

GitHub只读复核仍为远端HEAD `3cc6fb7`；最新run `41024736353` 对应 **旧 `c460ab6`，7/7 success，2026-10-30完成**。当前未提交变更没有对应远端SHA/CI，不把旧成功当当前验收。`cli-github-status.md`列出清单。**本次查询切片、本机检查和下一步计划完成，整体计划仍有上述未完成项。** 此前章节均为历史记录，以本节为最新状态。


## 2026-10-31 续实施：前置 HTTP 取消（最终本机验收）

用户再次核对进度/TDD/GitHub后，父agent选择交班中的前置HTTP取消缺口继续实施。实施及专项修订使用 **DeepSeek V4.1 Flash（high）**；父agent串联、双轴审查、修正夹具截止时间与受控竞态，并完成集成验收。仍以 `3cc6fb7f29167bd722487c5fde0008c29f3f63ae` 为基线，所有新增代码在工作区，未提交、未推送，无tag/release、真实业务账号/凭据/书库操作。

- `HttpPushBackend` 的 users/me、storage、bucket、STS、签名链接Range GET现使用可取消的owned async future；保持用户请求超时、原有API信封/错误分类、签名算法和最多三次只读重试。每个新请求前检查取消，停止后续阶段和第二文件；旧blocking兼容接口保留。
- 新增7项公共测试：6项真实CLI SIGINT覆盖五个阶段及API信封body悬停，另1项通过公开backend精确设置取消标志后立即放行成功响应，证明不继续storage。fixture处理已排队连接并真正join全部线程，逐次read检查绝对截止时间。
- TDD实际test-first Red为最初auth慢请求：目标已收到，SIGINT后8秒watchdog仍超时。其余共享机制用例直接Green；实现后的基线五失败对照和中间信封解析回归分别记录，不冒称逐项test-first。证据见 `tdd-prewrite-cancel.md`。

最终 `CARGO_NET_OFFLINE=true bash scripts/check_cli.sh` **全部通过：221项Rust、7项Python，0失败**；fmt、严格clippy、release、normal依赖边界、无显示启动、打包及产物运行全部通过。日志 `/tmp/sandflos-cli-prewrite-final-check.log`。新增7项最终集成日志 `/tmp/prewrite-parent-integration.log`。没有重跑200MiB性能，也没有将旧测量当作本轮结果。

规范与方案报告分别为 `cli-prewrite-standards-review.md`、`cli-prewrite-spec-review.md`，均有父处置：生命周期、body-stall/取消竞态与helper共享缺口已关闭；已有后写入错误映射重复作为维护判断保留，未扩大其语义变更。

GitHub最终只读复核：远端cli HEAD仍为 `3cc6fb7`，最新run `41024736353` 对应 **旧代码 `c460ab6`，7/7 success**，完成于2026-10-30；不覆盖当前未提交代码。`AGENTS.md`禁止提交/推送，当前变更三平台CI未运行，详见 `cli-github-status.md`。本轮本机实现/检查已完成，**整体清单仍不能标作全部完成**。

剩余：当前变更远端CI；桌面app-owned状态迁移及人工回归；query/queue等其他blocking路径、登录上游和旧桌面兼容路径；DNS/连接建立阶段的系统等待未单独验证；真实平台凭据/扫码、生产BOOX/OSS/WebDAV、设备接收和真实tag事件链。push上述前置HTTP取消本身已补齐，不再整体标为缺失。此前章节为历史证据，以本节为最新状态。

## 同日此前核对：三个切片的进度、TDD 与 GitHub

本轮三个实现切片与审查均已完成，续接没有重复实施或重跑已通过测试。DeepSeek V4.1 Flash（high）核对TDD公开行为Red记录与最终Green证据并修正 `cli-plan-status.md` 的旧CI/进度描述；审计见 `cli-progress-tdd-audit.md`，历史Red仅有文档记录、不能用最终Green日志反推的限制已明示。

只读GitHub查询确认：cli远端HEAD仍为 `3cc6fb7`；最新列出的CLI run `41024736353` 对应旧代码 `c460ab6`，7/7 success（2026-10-30完成）。当前工作区未提交，尚无覆盖新增代码的GitHub CI。详见 `cli-github-status.md`。AGENTS.md明确禁止提交/推送，已请求允许父agent提交推送以完成当前变更远端验收；授权前不执行。**本轮本地实现及检查完成，整体计划和当前变更CI不能标作全部完成。** 桌面迁移与真实平台/生产验收仍是后续范围。


## 本轮续实施：登录交付、连续读取进度与后写入取消（2026-10-30）

基线 `3cc6fb7f29167bd722487c5fde0008c29f3f63ae`，仍位于 `cli`，本轮改动保留在工作区，**未提交、未推送**。实施与专项修订使用 DeepSeek V4.1 Flash（high），父 agent 串联、复核与集成。保留了续接时已有的 auth.rs 和三份未跟踪公共测试，没有使用真实账号、凭据、服务器或用户书库；没有 tags/releases。

- `auth::LoginPortal::start` 提供应用显式令牌交付与 owned portal；QR、微信回调、手机/邮箱统一交付，失败可重试、成功一次。shutdown/drop 实际 join；半截请求和旧入口替换均有行为 Red/Green。应用回调必须有界返回；已经开始的上游请求仍可能使 shutdown 等待原有 20 秒请求超时。旧 `start_login_flow` 兼容入口保持。
- `PushProgress::Read` 与默认兼容的 `PushBackend::upload_with_progress` 接入实际 PushService 和 CLI。25 ms 采样本地读取计数，读取与 OSS 确认分开；16 MiB 慢读有多次单调有界样本，取消后停止回调。独立 HMAC 断言保护 OSS canonical resource；`Uploaded` 仍只在成功响应后产生。
- `HttpPushBackend` 消息写入、同步确认、最终提交使用 owned async future 取消，25 ms 检查并有最多 100 ms 在途响应收尾窗口；取消后不启动下一网络请求或下一文件。保留已收到 ID/rev 和确认成功，未确认写入/提交仍 unknown，不盲重试。无 detached blocking worker，用户超时原值保留，legacy `core/push.rs` 保持兼容。

本机 Linux 最终 `CARGO_NET_OFFLINE=true bash scripts/check_cli.sh` 通过：**214 项 Rust、7 项 Python，0 失败**；fmt、严格 clippy、release 构建、依赖边界、无显示启动、打包与产物运行全部通过。日志 `/tmp/sandflos-cli-round-check.log`。没有执行新提交的远端三平台 CI，后文旧 run 仅代表旧提交。没有重新测量 200 MiB 性能，不把旧测量冒称本轮实测。

行为证据：`tdd-login-delivery.md`、`tdd-read-progress.md`、`tdd-postwrite-cancel.md`。双轴审查与父处置见 `cli-round-standards-review.md`、`cli-round-spec-review.md`。规范轴由子 agent 返回、父 agent 处置。方案轴最初未及时返回，由父 agent 补验；续接时已由 DeepSeek V4.1 Flash（high）完成独立静态复核并追加报告，未确认本轮范围内缺陷。续接只调整审查/交班文档，未重复实施或重跑已通过的测试，`git diff --check` 通过。规范报告的签名遗漏两项已由最终源码与独立协议测试反证；文档中消息身份的汇总歧义已改正，保留历史 Red 与当前契约的区别。

尚未完成：桌面 app-owned 状态与调用迁移；认证、bucket、STS、签名下载链接等 blocking HTTP 的及时取消；真实平台凭据/扫码/桌面人工回归、生产 BOOX/OSS/WebDAV 与设备接收；本轮三平台 CI。登录与连续进度接口现已补齐，不再将这两项本身标作缺失，但不能据此声称桌面迁移已完成。


## 续实施：上传性能与慢连接取消（2026-10-30）

本轮起点为 `406ef53720dabf1df412359558dc8b8bc2002940`，继续使用 DeepSeek V4.1 Flash（high）子 agent，父 agent 集成审查。**最终代码/测试提交 `c460ab66704e4d253b954fc85d089c420d04a3a8`，对应 CLI run `41024736353`，7/7 成功**，已推送 `origin/cli`。产品实现为 `18d9d55`，`c460ab6` 修复 TCP 分段下的测试前缀读取。其后仅更新交班文档。下方此前的 `db3903b`/run `41014099241` 是历史验收，不能证明本轮改动。

最终远端验收：Linux/macOS/Windows 三个 CLI job 的测试、lint、release 构建、无显示运行与版本化 ZIP 验证全部通过；三平台从 GitHub 下载产物后校验并运行均通过；macOS 桌面兼容编译通过。Windows 不执行 Unix SIGINT 用例；不以构建通过替代真实平台/桌面人工验收。

- `cli-upload-performance.md`：新增可复现的 Linux 8/200 MiB 真实 CLI 回环上传基准，核对长度、SHA-256、单次 PUT 和 accepted；测量子进程采样 VmHWM，明确最后采样间隔和本机环境限制。
- `cli-slow-cancellation.md`：慢速 OSS 写背压取消的行为 Red/Green 与修复范围；只有 OSS PUT 使用可取消异步边界，其余阶段继续保留既有确认/unknown 语义。
- `cli-state-migration.md`：桌面全局状态调用链及迁移评估；本轮未迁移桌面状态，登录交付、连续进度和平台人工回归仍需后续处理。

本机最终 `CARGO_NET_OFFLINE=true bash scripts/check_cli.sh` 已通过：**198 项 Rust 测试、7 项 Python 测试，0 失败**，fmt、严格 clippy、release 构建、依赖闭包、无显示启动、打包运行通过。日志 `/tmp/sandflos-cli-next-final-check.log`。新构建的 release 二进制完成 8/200 MiB 和缓冲对照测量；200 MiB 采样 VmHWM 为 6.156 MiB，精确内容与单次请求验收通过。基准的无效 CLI 输出 2 MiB 对照在 2.64 秒内被拒绝，无管道死锁；没有将这些已有/审查补验行为补造为 Red。

规范轴与方案轴审查分别见 `cli-next-standards-review.md`、`cli-next-spec-review.md`，各自包含父 agent 处置。客户端构造两行策略与独立夹具的协议样本重复作为判断项保留，未宣称全部维护债务消失。

当前仍需后续处理：桌面 app 持有状态的实际迁移；非 OSS 阶段阻塞 HTTP 的取消上限；真实平台凭据/ACL、扫码和桌面人工回归；生产 BOOX/OSS/WebDAV、设备接收和真实 tag 到 draft release 事件链；生产及其他平台性能。200 MiB 本机回环采样已经完成，不再整体标为“未测”，但未证明无采样遗漏的绝对峰值或生产吞吐。

## 此前已完成的验收

更新：2026-10-30。仓库 `/home/fallingstar10/shire/sandFlos`，分支 `cli`。本轮从 `4d6a48a139851c7f169ed83bd0e1055031d86587` 继续，按原方案及已确认的 S1–S4 由 DeepSeek V4.1 Flash（high）子 agent 实施，父 agent 串联、集成与审查。

本轮最终代码与测试提交为 **`db3903bfa16f3cb2dc26d37cf97e6dcbebccc123`**，已推送至 `origin/cli`；对应 CLI workflow **run `41014099241`，7/7 成功**。其中产品实现提交为 `73f36f8`，`db3903b` 修正跨平台路径测试。`7c2e4fc` 是中间提交，不能单独当作最终验收。后续收尾提交只更新文档，不改变这份已验证代码。

| 最终远端检查 | 结果 |
| --- | --- |
| Linux、macOS、Windows CLI 测试、严格 lint、release 构建和无显示运行 | 3/3 成功 |
| 三平台版本化 ZIP 的生成、校验与运行 | 在各 CLI job 内通过，六份原始包/版本化归档 artifact 均已上传 |
| 从 GitHub 下载产物 ZIP 后校验并运行原生二进制 | 3/3 成功 |
| macOS 桌面兼容编译 | 1/1 成功；不替代桌面人工回归 |

此前 `49117e6` 的 run `40978370161` 只证明旧版本。首次本轮 run `41013482622` 在 macOS/Windows 的新增多库路径断言失败：实现返回 canonical 路径，测试期望却保留原始临时目录拼写。修正测试后，最终 run 通过；详细记录见 `tdd-boundaries.md`。

## 当前功能与本轮补齐

M1/M2 的命令已实现：认证、独立配置、JSON/退出码、本地推送、队列、设备/空间/阅读统计、Calibre、Zotero/WebDAV、迁移与诊断。完整命令见 `docs/cli.md`；逐项完成状态与未验收项见 `cli-plan-status.md`。

本轮重点：

- HTTP 改为类型化错误，保留状态/业务码；错误不带原始响应体或 URL。401/403、429/5xx、业务错误、损坏信封与响应体截断保持正确类别。成功信封必须存在 data；队列必须有 results 数组，不能用空队列吞掉无效响应。
- 重推准备阶段保留认证错误；消息写入、同步和提交不明保留 unknown 与已知身份；非幂等写入只提交一次。同步 revision 不再进入错误文本。
- 过期 STS 在建立上传连接前拒绝；实际 OSS PUT 内容/长度与确认丢失都有验收。上传进度只在 OSS 确认后发出一次，不把读完文件当作远端接受。
- SIGINT 使用可取消 reader；预检、阶段和批次检查同一取消信号。消息响应已到时保留 id/rev；最终提交已确认则保留 accepted，未确认才 unknown；后续文件不启动。真实子进程测试覆盖 8 MiB 慢上传和 saveAndPush 响应丢失，退出 130。
- 凭据锁冲突、注销失败、Windows 实际替换失败、跨年日期/不同时区/null 统计、QR 缺 token/拒绝/超时/取消/断网均有边界验收。Windows 专属替换用例由 Windows CI 执行，Linux 不将它记作已跑。
- Calibre 多库同 ID、全局分页/搜索、完整必需表列校验、500 ms 锁等待、损坏数据库与源字节不变；Zotero 本地/WebDAV、404、凭据不可读、ZIP 损坏/展开超限/符号链接与清理；迁移失败修正后由新进程验证凭据仍可用。
- 固定 Rust 1.97.1；新增独立 cli-v* 发布 workflow、版本化 ZIP 与外部 SHA-256。普通 CLI 三平台 CI 也构建并运行版本化 ZIP，无需创建真实标签。

此前依赖隔离、显式回环端点、权限受限文件凭据、原子替换、队列两步删除、WebDAV 路径/大小限制和临时文件清理继续保留。

## 最新本机验收

Linux 执行：

```bash
CARGO_NET_OFFLINE=true bash scripts/check_cli.sh
```

结果：**196 项 Rust 测试、7 项 Python 进程测试，0 失败**；fmt、严格 clippy、release 构建、CLI normal 依赖闭包、无显示启动、打包与原生二进制运行均通过。随后用本次新构建的二进制生成版本化原生 ZIP，并通过同一消费端校验器。临时日志 `/tmp/sandflos-cli-final-check.log` 不属于交付产物。

统一检查首轮的 Rust/Python 测试已全绿，但 clippy 指出新 SIGINT fixture 的嵌套 if；合并等价条件后完整重跑通过。此 lint 整理不是 TDD Red。

测试使用合成凭据和回环 HTTP、真实 CLI 子进程、临时真实 SQLite；没有访问真实账号、设备、用户书库或平台 keyring。真实 SIGINT 用例限 Unix，Linux 另观察子进程读取计数；Windows 由专属文件句柄替换测试补其平台边界。

## 审查与证据

- `cli-review-followup.md`：规范轴与方案轴分别报告、逐项处置；包含队列空结果误判、Calibre 表结构碰撞、成功确认被取消覆盖和预检取消优先级的修复。
- `tdd-http-errors.md`：类型化 HTTP、信封、队列结构、重推分类、STS 到期的实际 Red/Green。
- `tdd-transfer.md`：取消后的已知消息 ID、真实上传/提交 SIGINT、预检；记录 64 MiB 慢链路实验的限制。
- `tdd-boundaries.md`：书库、WebDAV 与迁移；父 agent 对 Calibre 必需列的追加修复。
- `tdd-calendar-storage.md`：日期、凭据持久化失败与 QR 失败路径。
- `tdd-output-secrets.md`：同步错误脱敏、上传确认丢失与公共进度契约。
- `tdd-release.md`：独立 CLI 发布、工具链和归档验收。

已有实现新增测试首次直接 Green 时明确记录为回归；不补造 Red 历史，也不将编译/下载错误计为行为 Red。早期证据继续见 `tdd-http.md`、`cli-libraries-execution.md`、`cli-artifact-verification.md`；旧数量不替代本节最终数量。

## 尚未完成的验收与架构债务

1. macOS Keychain、Windows Credential Manager 的真实写读、Windows ACL、终端二维码手机扫码。
2. 生产 BOOX/OSS/neocloud、真实 WebDAV、上游业务码及设备实际接收。CLI 的 accepted 表示云端接受，不能声称设备已收到。
3. 200 MiB 峰值内存/吞吐与生产慢链路。64 MiB 慢回环实验曾暴露 socket 写背压；v1 取消依赖 reader、阶段检查与有限请求超时，不能保证瞬间打断已阻塞的系统调用。8 MiB 测试不是大文件性能结论。
4. 桌面人工回归；macOS CI 只执行桌面 cargo check。
5. 真实 cli-v* tag 事件到 draft release 的完整链路。本轮不创建 tag、不发布 release；普通 CI 验证三平台归档脚本与原生产物。
6. 上游统计时区尚未确认，明确输出 timezone:null。请求日期不因本机时区转换，缺失统计保留 null。
7. CLI 已独立于桌面状态，桌面兼容入口仍保留 core 全局状态；原设计“最终由 app 持有状态”的全面迁移尚未完成。

上传进度 v1 为阶段/OSS 确认事件，没有连续百分比；zotero detect 明确不支持，使用显式目录；--timeout 是单次请求超时，读操作轮询可能有多次请求。无可用 keyring 的服务器用 SANDFLOS_TOKEN 或显式 SANDFLOS_CREDENTIAL_STORE=file，不静默明文降级。

## 下一位 agent

1. 先检查 git status/log，保留工作区；以本文件最终代码 SHA 和 CI run 为准，不重复引用旧 run。
2. 读 cli-plan-status.md 和相关切片记录，区分已完成自动化与上述人工/架构事项。
3. 新修复继续走已确认 S1–S4 的真实 Red/Green；已有行为回归直接 Green 诚实记录。
4. 不使用用户真实账号或书库作为日常 fixture，不把实际发布标签当测试资源。若改代码，运行适当检查并等待新提交对应 CI。
