# CLI 发布整改执行与人工验收

2026-11-04；实施基线 `f68a8fb`。父负责集成、审查和提交；三个 DeepSeek V4.1 Flash / high 子 agent 并行实施。范围仍为纯 CLI，不恢复桌面。历史审查见 `cli-release-readiness-2026-11-04.md`。

## 实施计划与已确认测试边界

沿用已确认 S1 CLI 子进程、S2 core 公共接口、S3 合成回环协议和 S4 安装产物；不新增未协商测试边界。每片先一个可运行行为 Red，再最小修复、回归和审查。编译/setup 失败不记 Red，不回滚已工作实现补造历史。

1. 认证取消与并发迁移：取消前后持久化一致；目标已存在不可被竞态覆盖；普通 import 保留显式替换语义。
2. queue 退出码与设备候选地址：写前取消 130、写后 unknown 6；push 原有 130 保留；ARP-only 地址不丢失，不声称已验证设备服务。设备地址部分已完成：`build_dashboard_devices_with_environment` 在 API 无 IP 时回退到匹配 MAC 的 ARP 地址，两者都存在时优先使用 MAC 匹配的 ARP 地址，避免云端公网 IP 压过已知局域网候选；公共测试 `crates/core/tests/device_contract.rs`（`arp_only_address_remains_a_transfer_candidate`、`arp_candidate_wins_over_a_nonlocal_cloud_address`），命令 `CARGO_NET_OFFLINE=true cargo test --locked -p boox-core --test device_contract`，退出 0。初始 Red 日志 `/tmp/booxctl-device-arp-red.log`（`transfer_host` 为 `None`），Green 日志 `/tmp/booxctl-device-public-final-green.log`；优先级行为的 Red/Green 日志为 `/tmp/booxctl-device-finish-red.log`、`/tmp/booxctl-device-finish-green.log`。候选地址沿用现有默认 HTTP 端口，ARP 不提供设备服务端口或在线证明；真实设备服务可达性与实际接收仍未验收。未使用真实设备或生产 WebSocket。定向查询回归 `query_cancel` 11 项、`query_queue_e2e` 4 项、`query_validation` 4 项通过，日志 `/tmp/booxctl-device-targeted-check.log`；core 定向 clippy 通过，日志 `/tmp/booxctl-device-clippy.log`。并行工作区中的 auth QR 时序测试和 `library_cancel` clippy 未由本片处理。
3. Zotero：配置与凭据能力准确投影；WebDAV 网络等待取消与清理在可控回环中验收。
4. 文档与发布：修正 README、明确升级步骤及有限支持矩阵；固定三个 runner 版本，增加 release workflow 的 cli push 演练入口，manifest 取版本，只打包不发布。
5. 父统一检查、审查、提交及 GitHub 当前 SHA 构建核对。完成一个切片不代表正式发布全部完成。

## 必须人工配合的门槛（当前均未验收）

| 项目 | 需要提供/确认 | 验收方式与证据 |
| --- | --- | --- |
| 平台凭据 | macOS / Windows 测试机器及独立配置目录 | 跨进程登录/退出、拒绝访问、钥匙串隔离；文件后端 ACL；记录 OS 与 SHA，不记录秘密 |
| QR 与账号 | 测试账号、手机 BOOX 助手 | 可扫描、确认/拒绝/过期/取消；取消后不新增凭据 |
| 云端与设备 | BOOX 测试设备和允许执行的测试文件 | 上传→accepted→设备实际收到分开记录；重推/删除只处理测试记录 |
| Zotero/Calibre/WebDAV | 可用的测试书库与 WebDAV 服务 | 记录版本；本地/远程附件、401/404、取消清理；不使用真实私人书籍作 fixture |
| Windows 信号 | 真实交互控制台 | Ctrl-C 写前/写后结果及退出码；Unix 信号测试不代表 Windows 通过 |
| 默认分支发布策略 | 维护者确认如何将 release workflow 放入默认分支 | 不擅自改默认分支或把 CLI 合入保留桌面的 main；cli push 自动演练无需此步骤；若需要手动 dispatch，再确认后部署工作流并 dispatch 指定 SHA |
| 真正发布 | 明确允许创建版本标签和 draft/release | 最终 SHA 的 tag→三包→draft→人工检查；代码提交授权不等于标签/发布授权 |

每次验收记录：日期、SHA、OS/CPU、设备/应用版本、操作、预期、实际、退出码、脱敏日志、验收人。失败项保持未完成；不得仅据 CI 全绿勾选真实验收。

## 技术债处置

- v0.1 暂不扩平台：Linux x64/glibc 2.39、macOS ARM64、Windows x64；固定 Ubuntu 24.04 / macOS 15 / Windows Server 2025 runners。最低运行环境需遵守文档，其他系统保持未验证。
- 旧 CLI 不自动跨钥匙串迁移：提供重新认证/配置及文件后端复用旧目录指引，保留旧数据。
- 旧桌面全局包装、helper 去重、包管理器、签名/公证及自动更新后置，不为清零技术债大规模重写。
- 阻塞文件系统/DNS/平台凭据系统调用不能保证即时取消；不得用 detach 伪造及时退出。各切片报告列出实际剩余限制。

## 最终证据

本轮保留并完成进入工作区时已有的修订，使用 DeepSeek V4.1 Flash / high 实施，父审查收口。跟进消息一度不可读，改用本地审查文件继续；父最终接管整合。

- S1/S2：认证保存前取消检查、Unix stdin 可取消及标志恢复、二维码阶段取消、迁移锁内拒绝覆盖；文件/钥匙串写入参与互斥。真实钥匙串验收仍待人工。
- F1/F3/R4：queue unknown 退出 6、写前取消 130，push 原契约保持；WebDAV 列表能力投影、下载响应头/体取消、容量限制和清理。
- F4：ARP-only 与陈旧云端 IP 的候选选择；探测命令两秒期限、有限输出捕获和直接子进程回收。候选地址不证明服务在线。
- S3/F2/R1/R2：此前 `46fad34` 已完成说明/升级/安装/支持矩阵及 cli push 三平台打包演练；对应 CLI run `41761845041` 成功、release rehearsal `41761845065` 成功。

本轮 TDD 审计：[认证](tdd-release-auth.md)、[队列与书库](tdd-release-queue-library.md)、[设备](tdd-release-device.md)。继承代码缺失的 Red 证据明确标记；不回滚实现补造历史。统一检查、提交和新 SHA 的 CI 结果见下方记录。


### 本轮本机统一验收

`CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true bash scripts/check_cli.sh`：退出 **0**，**267 Rust + 7 Python** 通过；fmt、严格 clippy、release 构建、依赖边界、无显示 smoke、打包和消费端校验全部通过。最终日志 `/tmp/booxctl-release-final-check2.log`。首轮测试全绿但 clippy 报 needless_return（退出101），等价修正后完整重跑；不是行为 Red。

最终审查：已修复代码层 S1/S2/F1/F3/F4，R4 的 WebDAV 网络等待已可取消；Windows stdin、平台系统调用、ZIP内存上限和真实业务验收限制仍明确保留。S3/F2/R1/R2 的文档及自动演练由前序提交完成。上表人工验收和真正 tag→draft 未完成，不能宣称正式发布完成。当前代码提交后的 GitHub run 需要按精确 SHA 核验；远端结果见本次交付消息，下一轮以实际 HEAD 重查。


### 首次远端验证与修正

`701069c` 的 CLI run `41831644967`、打包演练 `41831644897` 在 Windows 严格 clippy 失败：设备临时目录 builder 的可变性仅 Unix 需要，Windows 报 `unused_mut`。Linux/macOS 打包通过；Windows 测试本身通过。父将可变 builder 限定在 Unix cfg 内，不改业务行为。失败日志 `/tmp/booxctl-ci-701069c-failed.log`。修正后的 run 仍需按最终 SHA 核实，不能用这次部分通过代替。


### 2026-11-05 最终远端验收

验证代码 SHA：`76a42f01a0f9a9ad8ffd5c261c55ef0bec23c211`。

- CLI run `41875934420`，attempt 2：**success，6/6**。Ubuntu 24.04、macOS 15、Windows 2025 的 CLI 检查及下载产物复验全部通过。
- CLI release run `41875934417`，attempt 2：**success，三平台 package 3/3**；分支演练不创建标签/release，draft-release 按设计 skipped。
- attempt 1 的 macOS 任务因托管 ARM64 runner 容量不足未取得执行器而 cancelled，不是代码测试失败。2026-11-05 父执行 `gh run rerun 41875934420 --failed` 和 `gh run rerun 41875934417 --failed`，均退出0；复用已通过的 Linux/Windows 任务后完成所有剩余检查。
- 额外下载 Linux artifact：`gh run download 41875934420 --name booxctl-ubuntu-24.04 --dir /tmp/booxctl-76a42f0-linux`；`python3 scripts/verify_cli_package.py /tmp/booxctl-76a42f0-linux` 退出0，输出 `Verified CLI package: booxctl`。
- 原始状态快照：`/tmp/booxctl-ci-76a42f0-final.json`、`/tmp/booxctl-release-ci-76a42f0-final.json`。临时日志可能被清理，本节保存长期结论与可重查的 run ID。

自动化发布整改与三平台验收已完成；上表真实平台/账号/设备/Windows 控制台和 tag→draft 验收仍未完成。后续仅更新本记录的文档提交使用 `[skip ci]`，其产品代码与上述已验证 SHA 完全相同，不将文档提交冒称具有独立 CI run。
