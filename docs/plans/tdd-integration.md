# CLI 集成验证记录

日期：2026-10-29。用户授权按 `cli-review-and-tdd.md` 实施，S1–S4 为公共测试边界。实施和后续补齐使用 DeepSeek V4.1 Flash / high 子 agent；协调 agent 负责接口衔接、书库、失败语义、集成和审查。已有实现的回归测试不冒充 Red/Green。

## 实现与审查

- 独立 `sandflos-cli`，二进制 `sandflos`；默认 workspace 成员为 core 与 CLI，autostart 依赖留在桌面 app。
- CLI 独立认证、配置目录、JSON 信封、错误码与逐文件报告；业务服务不刷新仪表盘，不读桌面全局凭据。
- HTTP 端点显式注入，CLI 测试设置仅接受字面回环 origin；API-only 使用同源 OSS，无效配置不能回退线上。认证入口采用相同限制。
- 上传配额查询失败即停止；只读查询和队列不依赖存储统计接口。`saveAndPush` 和重推写入不盲重试，写消息/同步/最终提交结果不明保留 unknown；重推的 404 重建保留原格式。
- neocloud 复用调用方 HTTP 客户端，继承请求超时和重定向策略；同步 Cookie 按实际请求 URL 的 domain/path/secure 规则选取。
- 队列删除保留两步结果、404 幂等、部分完成退出 6；本地凭据错误保持退出 5，取消标记传入变更后端。
- Calibre、Zotero 使用只读 SQLite 与显式目录；WebDAV ZIP 必须包含所选附件，含空格密码可存取，下载限制、路径检查和临时清理保留。
- 打包和消费端验证共用脚本；验证内容、SHA-256、可执行位和无显示启动，拒绝额外文件、符号链接与危险归档；`--force` 不覆盖无关文件。

## 测试证据

- 书库实际 Red/Green：`cli-libraries-execution.md`。
- HTTP、CLI 失败语义与实际 Red/Green：`tdd-http.md`。
- 凭据与早期认证：`tdd-auth.md`；早期服务：`tdd-services.md`。
- 打包、下载验证：`cli-artifact-verification.md`。

交班基线提交 `3d79aef` 的 121 项 Rust 测试、本地检查、Linux/macOS/Windows CLI CI 与 macOS 桌面编译均通过。本轮最新 Linux 统一检查已通过：158 项 Rust 测试、5 项 Python 进程测试，0 失败；fmt、严格 clippy、release、依赖边界、无显示 smoke、打包与本地产物运行均通过。

2026-10-30 收尾：夹具修复提交 `25aeab2` 的 CI run `40976698508` 已全部通过，包含三平台 CLI 检查、三平台下载包验证/运行和 macOS 桌面编译，共 7 项。间歇 CI 失败暴露的回环连接模式问题已修复：非阻塞 listener 接受连接后显式设置阻塞读写与超时，WebDAV 夹具读取完整请求头；原业务断言保留。精确交班记录见 `cli-handoff.md`。

同日最终审查进一步补齐 WebDAV 下载后推送的真实 CLI 组合回归：成功退出 0、最终提交未知退出 6，均检查上传字节、实际报告路径的文件/目录清理和源数据库不变。实现此前已有，不新增业务 Red/Green 声明。最终验收代码为 `49117e6`，其 CI run `40978370161` 已 7/7 全部通过，覆盖上述三平台检查、下载包运行和桌面编译。

统一命令：

```bash
bash scripts/check_cli.sh
```

它执行 Python 进程测试、rustfmt、两 crate Rust 测试、严格 clippy、release 构建、依赖闭包检查、无显示 smoke、打包与本地产物运行验证。workflow 额外在三平台下载本 run 的 artifact ZIP，再在对应 runner 验证并运行。

## 验收边界

HTTP 夹具使用合成协议数据，未抓取生产样本。真实 BOOX 账号、云端业务码、设备接收、平台凭据写入/读取、Windows ACL、二维码可扫码性、大文件实际内存/吞吐、慢链路与取消 smoke 仍需单独验收。云端接受与设备收到分开记录。CI 文件存在不代表本轮 CI 已通过；精确提交和 run 以交班记录为准。
