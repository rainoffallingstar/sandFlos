# booxctl 发布就绪度审查（2026-11-04）

结论：核心命令和自动化构建已具备，但当前版本不宜标记为完成正式发布验收。存在可复现的认证取消和迁移并发问题、退出码契约冲突，以及发布入口、升级说明和平台验收缺口。本次只审查并保存报告，未修改生产代码、提交、推送、创建标签或发布。

## 范围与证据

- 当前分支 `cli`，HEAD `f68a8fb09ee7b75e8171ce715f0d317f066b8498`；开始时工作树干净。
- 近期改动基线 `e9ff2eeb709c01d4e29573757572d92a8e51d352`，命令 `git diff e9ff2ee...HEAD`；同时检查现有 CLI 全链路，明确区分既有缺陷、改名兼容缺口和未验收项。
- 规范：`AGENTS.md`、`docs/plans/cli-review-and-tdd.md`；当前对外契约：`docs/cli.md`。已终止的桌面迁移、窗口和托盘验收不计入 gap。
- 两位只读审查者分别检查 Standards / Spec，父审查发布链路，并复核关键发现。以下两轴分别保留，不用某一轴通过替代另一轴。
- `CARGO_NET_OFFLINE=true bash scripts/check_cli.sh`：退出 **0**，**255 Rust + 7 Python** 通过；fmt、严格 clippy、release 构建、依赖边界、无显示 smoke、打包与校验通过。日志 `/tmp/booxctl-release-readiness-20261104.log`。没有重新运行 200 MiB 性能基准。
- GitHub API 只读核实 run **41260826105**：2026-11-01 创建，SHA 正是上述 HEAD，CLI / verify-package 三平台 **6/6 success**。日志下载到 `/tmp/booxctl-ci-review-logs.zip`。
- 下载 Linux artifact **13064587998** 至 `/tmp/booxctl-ci-linux-41260826105.zip`；使用现有 `verify_cli_package.extract_zip/verify` 复验成功，再以 `readelf --version-info` 检查 ABI 要求。
- 全部行为探针仅使用临时目录、合成 token 和字面回环地址；无真实账号、用户书库、系统钥匙串或业务请求。临时证据文件可能被系统清理。

## Standards

### S1 · P1 · 认证取消后仍执行持久化（既有问题，父已复现）

`crates/core/src/auth_service.rs:815` 在验证返回后直接保存，没有取消检查；QR confirmed 分支 `:874` 同样直接保存。CLI 在 `crates/cli/src/main.rs:75` 最后才把退出码改为 130。违反原方案第 97 行“停止后续副作用”的取消要求；文档允许等待当前阻塞请求超时，并不意味着允许随后保存。

探针在 `/api/1/users/me` 已到达回环服务器后发 SIGINT，再返回成功响应：进程退出 **130**，stdout 却为 `ok:true, authorized:true`；新进程 `auth status` 仍为 authorized，证实取消后发生凭据写入。QR 分支为静态确认，未执行交互扫码复现。审查者另外观察到未关闭的 stdin 管道会使 `auth import` 在 SIGINT 后继续等待 EOF；该探针使用已有 debug 二进制，未保存独立日志。

实施：让认证入口接收取消状态；阻塞请求返回后、任何保存之前重新检查；为 stdin 输入定义有界取消行为；确保 JSON、退出码、实际凭据状态一致。不要通过 detached 阻塞 worker 解决。

### S2 · P2 · 并发迁移突破“目标已存在拒绝覆盖”（既有问题，父已复现）

`crates/cli/src/config.rs:17` 先检查目标为空，随后读取源和联网验证；`crates/core/src/auth_service.rs:485` 仅在最终 save 内取得短期锁，没有在同一原子操作中重检目标。违反原方案 T12 的“目标已存在拒绝覆盖”。

探针让两个迁移都进入验证请求，再依次放行：两者退出 **0** 并声明 migrated，最终凭据变成第二份。锁保证单次写入不交错，但没有保证迁移的“不覆盖”语义。

实施：提供锁内检查与保存的 create-if-absent 用例；并发时第二个调用必须明确 busy / already-exists，且不能覆盖第一个结果。普通 auth import 的显式替换语义不应一起改变。

S1/S2 的父复现命令：`python3 /tmp/booxctl-readiness-probes.py`，探针退出 **0**（成功观察到缺陷，不代表产品正确）。脚本及日志分别在 `/tmp/booxctl-readiness-probes.py`、`/tmp/booxctl-readiness-probes-20261104.log`。未回滚实现、未将这些审查探针宣称为既往 TDD Red。

### S3 · P2 · booxctl 改名缺少旧 CLI 升级路径（近期兼容缺口）

默认配置目录、环境变量与 `crates/core/src/auth_service.rs:553` 的钥匙串 service namespace 一并更名。指定旧 `--config-dir` 也不能取得旧 service 下的钥匙串凭据，WebDAV 密码同样受影响。`crates/cli/src/config.rs:11` 的目录输入只寻找 `auth_cache.json`，当前迁移只导入旧格式 token，不迁移钥匙串和书库设置。

实施：补充显式旧 CLI 配置/凭据迁移，或明确本版为不兼容切换并给出重新登录、书库和 WebDAV 重新配置步骤。文件后端可复用旧目录的情况应与钥匙串后端区分；不要自动删除或改写旧凭据。若没有需要支持的旧 CLI 用户，升级说明可以代替兼容实现。这不是已承诺向后兼容规则的硬违反。

规范轴：**2 项硬规则发现 + 1 项兼容缺口**；最严重为 S1。没有单独列出可操作的代码异味。

## Spec

### F1 · P1 · queue unknown 的实际退出码与当前契约冲突（既有问题）

`docs/cli.md:20` 明确写前取消 130、可能写入但未确认 6；`crates/cli/src/main.rs:75` 却在收到 SIGINT 后无条件返回 130，覆盖 queue 层产生的 6。JSON 仍保留 unknown，但只依赖约定退出码的脚本不能区分两种结果。

`crates/cli/tests/queue_cancel.rs:551` 的公共断言显式要求 130，且被写入不确定场景 `:646`、`:678` 等复用。本轮全套测试通过恰好证明当前测试固化了该行为，并没有验证文档要求。

实施：统一并落实 queue 退出码优先级，覆盖“写前取消 / 写后 unknown / 已确认结果”的真实子进程测试。避免机械改变 push 已单列的取消契约；若决定调整公共契约，代码、文档与测试必须同时明确。

### F2 · P2 · 首页说明无法作为可靠入门路径（既有文档问题）

- `README.md:19` 和 `:63` 描述默认浏览器回环登录；CLI `crates/cli/src/auth.rs:193` 实际使用终端二维码。
- `README.md:29` 的裸 `devices`、`storage`、`reading` 均缺少子命令。本轮使用隔离配置实跑三条命令，全部退出 **2**。应分别使用 `devices list`、`storage status`、`reading stats --date YYYY-MM-DD`。
- `README.md:64` 称“分片上传”，实际路径为流式单次 OSS PUT，不提供 multipart / 断点续传承诺。

实施：校正首页及下载包说明，补解压、校验、放入 PATH 的安装步骤；快速开始示例按真实二进制验证。历史计划不必全量替换旧名，但当前状态入口必须清楚标注历史，避免重新拾起已取消桌面工作。

### F3 · P2 · Zotero 列表错误标记远程附件可用性（既有实现问题，静态确认）

`crates/core/src/cli_libraries.rs:227` 调用 `list_items_at` 时将 WebDAV 可用标志硬编码为 false；`crates/core/src/zotero.rs:894` 因而输出 `can_download_from_webdav:false`。即使已配置 WebDAV、push 可以下载，本地缺失附件仍被列表标记为仅“本地附件缺失”。

实施：将 CLI 自己的 WebDAV 配置/凭据状态接入可用性投影，区分“已配置可尝试下载”和“远端已核实存在”，避免列表暗示已经联网验证。补“同一配置下 list 与 push 能力一致”的公开行为测试。

### F4 · P2 · 局域网互传地址解析不完整（既有能力缺口，静态确认）

`crates/core/src/services.rs:1873` 为 CLI 传入空 share-device 投影；`crates/core/src/device.rs:246` 在构造 transfer_host 时只使用 api_ip，没有回退到已发现的 arp_ip。因此存在 `same_lan:true`、`lan_ip` 已知，但 `devices transfer-url` 仍报不可达的路径。

实施：明确可支持的地址来源和端口，修复 ARP-only 地址缺口，并为没有足够端口/服务信息的情形返回准确限制；不要仅凭同网段猜测服务可用。涉及设备发现的外部命令和探测也需要有界等待。未操作真实局域网设备。

方案轴：**3 项确认契约问题 + 1 项部分能力**；最严重为 F1。没有确认本次近期 diff 引入的新行为回归或范围膨胀。

## 父审查：发布工程与验收 gap

### R1 · 发布演练入口尚未接通

2026-11-04 只读查询：仓库默认分支是 `main`，其 `.github/workflows` 只有 `release-macos.yml`；已注册 workflow 列表为 CLI 与旧 Desktop，查询 `release-cli.yml` 的 runs 返回 **404**。

`docs/cli.md:58` 宣称可在 Actions 手动运行 CLI release，但 GitHub 官方文档规定 workflow_dispatch 文件必须存在于默认分支。官方说明已通过 HTTPS 读取核实，来源：`https://docs.github.com/en/actions/how-tos/manage-workflow-runs/manually-run-a-workflow`；网页工具未返回内容，随后直接读取官方网页。不能把仅存在于 cli 分支的 dispatch 声明等同于已可用入口。

实施：在确定的默认分支发布策略下提供 dispatch 工作流，再对目标 cli ref 做无标签、无 release 的打包演练。此发现只涉及手动入口，**不证明未来 push 标签触发一定失败**。真正 tag → 三平台构建 → draft → 维护者发布仍需单独验收；本次没有触发或创建这些资源。

查询命令（只读）：

```bash
gh api repos/rainoffallingstar/sandFlos --jq '{default_branch,private}'
gh api repos/rainoffallingstar/sandFlos/actions/workflows
gh api 'repos/rainoffallingstar/sandFlos/contents/.github/workflows?ref=main'
gh api 'repos/rainoffallingstar/sandFlos/actions/workflows/release-cli.yml/runs?per_page=5'
gh api repos/rainoffallingstar/sandFlos/actions/runs/41260826105/jobs
```

### R2 · 支持平台下限尚未定义

CI run 41260826105 的真实日志显示仅生成：

- `booxctl-v0.1.0-x86_64-unknown-linux-gnu.zip`
- `booxctl-v0.1.0-aarch64-apple-darwin.zip`
- `booxctl-v0.1.0-x86_64-pc-windows-msvc.zip`

下载的 Linux artifact 校验与启动通过，但 ELF 引用了 **GLIBC_2.39**；不能推断它可在更旧 glibc 系统上启动。Intel macOS、Linux ARM64 没有本轮构建产物。当前 `*-latest` 加 host target 的方式会使支持范围跟随 runner 漂移；`scripts/release_cli.py` 接受某个 target 名称并不等于 CI 已构建该 target。

实施：先明确 OS / CPU / libc 最低支持矩阵，再固定构建环境并在最低支持环境复验。若 v0.1 只支持当前三个 target，也可以明确限制，不必为“完整”无条件增加所有架构。包管理器、自动更新、补全不是现有发布契约的必选阻塞项。

### R3 · 真实平台和真实业务验收仍为空白

自动化 fixture 和 help/version 产物 smoke 不代替：

1. macOS Keychain、Windows Credential Manager，以及选择文件后端时的 ACL；跨进程保存、读取、隔离、退出、拒绝访问。
2. 终端二维码在实际终端可扫描，登录成功、拒绝、过期及取消；真实 token 导入验证。
3. 合成小文件经 BOOX / OSS / neocloud 推送、队列查询/重推/删除；云端 accepted 与设备实际接收分别记录。
4. Zotero / Calibre 受支持版本的书库，以及真实 WebDAV 认证、附件下载、临时文件清理。
5. 设备发现/互传地址，阅读统计的日期与上游时区；当前 timezone:null 是明确未知，不是已验证正确时区。
6. Windows Ctrl-C 的真实控制台测试。现有主要 CLI SIGINT 测试使用 `#![cfg(unix)]`，Windows CI 绿色不证明这些场景运行过。
7. 发布标签到 draft 的真实链路，以及下载发布包后的安装使用。

上述是未取得证据，不一概认定功能坏了。AGENTS 当前禁止真实账号、凭据、书库、tag/release；本次未执行这些动作，未来验收需另行明确授权与测试材料。

### R4 · 尚未覆盖的取消/资源限制

WebDAV 下载 `crates/core/src/cli_libraries.rs:395` 仍为阻塞请求，没有取消输入；`crates/cli/src/zotero.rs:177` 默认等待上限 90 秒。归档还会整体读入内存，限制为 200 MiB，不能将 OSS 流式上传的内存表现推广至 WebDAV。登录 HTTP、设备本地探测、DNS 和阻塞文件读取也不能由已完成的 push/query/queue 回环取消测试推断全部可及时中止。

实施：至少在发布说明明确阶段限制；若承诺整个 zotero push 可及时取消，需要补可取消下载、清理与有界回收测试。这与 S1“取消后继续新增持久化副作用”的确认缺陷分开处理。

## 建议实施顺序与完成门槛

1. **认证与迁移一致性**：S1、S2；以受控回环竞态建立真实行为 Red，修复并验证退出后状态，无 detached worker。
2. **脚本公共契约**：F1，明确并测试 queue 退出码；一起补 F3 和 F4 的可观察行为缺口。
3. **升级与安装交付**：S3、F2、R2；产出明确可复制的升级/安装指南和支持矩阵。
4. **发布演练**：R1；接通默认分支入口，对最终 SHA 完成三平台验证和原生包验收。
5. **真实验收与正式发布**：R3，记录目标 SHA、系统/设备/服务版本、步骤、结果及限制；处理发现后再进入 tag → draft → 发布。

可后置：纯内部的旧桌面兼容包装清理、取消 helper 去重、包管理器集成。已删除的桌面端不需要恢复，也不再列为发布前置工作。
