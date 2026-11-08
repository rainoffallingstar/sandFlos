# 无头服务器浏览器授权：TDD 方案与证据

2026-11-07。用户授权设计并实施；沿用既定 S1 CLI 进程、S2 core 公共登录用例、S3 外部 HTTP/WebSocket 协议边界（`cli-review-and-tdd.md`）。不增加桌面集成。

## 契约

- `auth login --url` 显式选择微信浏览器链接；默认 `auth login` / `--qr` 保持终端二维码。
- 允许无 TTY 和 `--no-input`：命令不读取 stdin，外部浏览器仍须人工授权。拒绝 `--json`，避免将含会话信息的链接混入自动化 JSON 契约。`--qr --url`、`--provider boox --url` 和重复模式参数均为参数错误。
- 服务端先建立出站 WebSocket 并确认 nonce 房间，再向 stderr 显示一次性官方授权链接；客户端浏览器访问官方页面及回调，服务端只等待结果，不开入站端口、不启动浏览器、不模拟手机确认。
- 令牌到达后走已有 AuthService 验证/保存，环境令牌生效时提前拒绝；取消、超时、坏消息及校验失败不得新增凭据。单次请求由 `--timeout` 限制，会话总期限五分钟。
- 授权链接包含一次性会话标识，仅在显式 `--url` 输出；不进入仓库/持久诊断日志，不输出授权码和令牌。

## 切片顺序

1. S1 非 TTY `--url` 能到达认证前置检查，而非旧参数/终端错误：先跑现有 CLI 的行为 Red，再添加 URL 用例和入口。
2. S1 `--no-input` 的链接流程及参数冲突，逐个 Red/Green。
3. S2/S3 合成浏览器确认→WebSocket令牌→HTTP校验→公开 status 可见；补充取消、超时、协议错误、验证失败与旧凭据保护。已由复用代码满足的回归如实记录直接 Green，不补造 Red。
4. fmt、完整测试、clippy、release 及产物检查；真实浏览器扫码另为人工验收，不能由 fixture 成功代替。

HTTP 读写与子进程等待必须有界，线程真实 join，所有凭据为合成值。新公共 API 的编译失败不算 Red。

## 实际切片记录

所有命令使用 `CARGO_NET_OFFLINE=true cargo test --locked`；以下日志在 `/tmp`，可能被环境清理。

| 测试命令（接上述前缀） | Red | Green | 日志 |
| --- | --- | --- | --- |
| `-p booxctl --test auth url_login_without_tty_rejects_environment_token_before_network` | 退出101，旧CLI返回usage而非环境凭据保护 | 退出0，1 passed | `booxctl-url-red1.log` / `booxctl-url-green1.log` |
| `-p booxctl --test auth url_login_no_input` | 退出101，错误地返回interactive_terminal_required | 退出0，1 passed | `booxctl-url-red2.log` / `booxctl-url-green2.log` |
| `-p booxctl --test auth url_login_rejects_conflicting_modes` | 退出101，冲突参数落入交互限制而非usage | 退出0，1 passed | `booxctl-url-red3.log` / `booxctl-url-green3.log` |

以下测试直接 Green，均退出0，各1 passed（复用既有认证和socket错误处理，不补造 Red）：

- `-p boox-core --test wechat_login url_login_matches`：链接state与已加入房间一致；浏览器确认后才发合成token，服务端仅请求用户校验、不再获取QR或重复浏览器回调；保存后公开status已认证。日志 `/tmp/booxctl-url-flow.log`。
- `-p boox-core --test wechat_login url_failures`：401、坏JSON、空token、关闭及超时不覆盖原来已验证凭据。日志 `/tmp/booxctl-url-failures.log`。
- `-p boox-core --test wechat_login url_cancellation`：链接展示前/后取消均不保存、不请求用户校验；展示前取消不触网。日志 `/tmp/booxctl-url-cancel.log`。

新增CLI预检子进程有10秒截止，失败kill后真实wait；回环浏览器等待有绝对deadline并响应stop，HTTP沿用有界夹具和真实join。协议字段预期取既有官方协议记录中的固定appid/scope/redirect URI；不调用生产URL生成器构造测试预期。

## 线上无 TTY 冒烟

`python3 /tmp/booxctl-url-smoke.py target/debug/booxctl`，使用独立临时配置、文件后端、DEVNULL stdin、管道 stdout/stderr，清除环境token和测试端点。脚本只在内存取链接并请求官方页面，随即SIGINT，不扫码；输出不含链接、state或令牌。

首次辅助脚本退出1、CLI退出4 / socket_error，尚未展示链接；日志 `/tmp/booxctl-url-live-smoke.log`。原样复验一次辅助脚本退出0：`no_tty_url_displayed=true`、`official_qr_page_accessible=true`、CLI退出130、`credentials_saved=false`；日志 `/tmp/booxctl-url-live-smoke-retry.log`。

该结果验证无TTY入口和官方页面可访问，不证明真实用户扫码、浏览器回调、真实token保存或设备收件通过。socket_error在此前二维码模式也出现过，根因未确定；没有为此加入自动登录重试。

## 最终验证（2026-11-08）

- 定向测试：`cargo test --locked -p boox-core --test wechat_login url_` 通过 3 项；`cargo test --locked -p booxctl --test auth` 通过 17 项（含 3 项新增 URL 入口用例）。
- 完整检查：`bash scripts/check_cli.sh` 退出 0，日志 `/tmp/booxctl-url-full-check.log`；其中 Python 脚本、Rust 全量测试（33 个 `test result: ok` 段）、`clippy -D warnings`、release 构建、依赖边界检查、打包与产物校验全部通过。
- 格式检查：`git diff --check` 与 `cargo fmt --all -- --check` 退出 0。
- 线上无 TTY 冒烟（release 二进制）：`python3 /tmp/booxctl-url-smoke.py target/release/booxctl` 退出 0；`no_tty_url_displayed=true`、`official_qr_page_accessible=true`、CLI 退出 130、`credentials_saved=false`。

以上冒烟只验证无 TTY 入口和官方授权页可达，未扫码；真实浏览器回调、微信确认、真实令牌保存与后续设备推送仍需人工验收。

## 提交与远端结果

- 代码与文档提交：`897a711`（`feat(cli): add headless WeChat browser login`），已推送 `origin/cli`。工作区在该提交后干净。
- CLI run `42437356734`：**success，6/6**。Ubuntu 24.04、macOS 15、Windows Server 2025 的 `cli` 检查与 `verify-package` 下载产物复验全部成功。
- CLI release run `42437356730`：**success，三平台 package 3/3**；`draft-release` 按分支演练设计 skipped，未创建标签或 release。
- 未验收：真实微信扫码与手机确认、真实浏览器到 BOOX 回调、真实令牌保存、目标设备收件。需要用 `--url` 的链接在本地浏览器完成一次人工登录后才算通过。
