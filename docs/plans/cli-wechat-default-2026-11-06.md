# 默认微信扫码登录（2026-11-06）

用户要求：本机二维码已可显示，希望 `auth login --qr` 默认使用微信。
范围为 core + CLI；没有桌面集成、标签或发布操作。

## 行为

- `auth login` / `auth login --qr` 默认微信；`--provider wechat` 显式选择微信，`--provider boox` 保留原 BOOX 助手方式。
- 固定官方微信/BOOX 端点；先建立 WebSocket 并确认房间，再取 XML 会话、解码 JPEG/PNG 并在终端显示。根据 XML 选择微信轮询域名；不执行响应中的脚本、不跟随重定向。
- 微信确认后调用固定 BOOX 回调，接收令牌，再交给现有 AuthService 校验和持久化；环境令牌生效时在发起请求前拒绝登录。二维码、微信码、state、令牌不进入持久日志。
- 终端使用黑白半格字符，当前 47 模块图片占 49 列；无需浏览器。整体会话 5 分钟，各请求受 `--timeout` 限制。阻塞 DNS/系统调用仍不能保证即时取消。

## 继承与 Red/Green

进入本轮时工作区已有未提交的 `wechat_login.rs`、图片依赖、测试草稿及协议记录；不声称这些实现拥有本轮 Red。先修正测试草稿中的矛盾断言、固定 nonce 假设和缺失 uid 等夹具错误。此类失败不是行为 Red。修正后的基础流程先通过（`/tmp/booxctl-wechat-red.log` 文件名虽含 red，实际是基线 Green）。

真实行为 Red：

1. `CARGO_NET_OFFLINE=true cargo test --locked -p booxctl --test auth explicit_qr_providers_reach_interactive_guard`：退出 101，旧 CLI 返回 `usage` 而非 `interactive_terminal_required`。日志 `/tmp/booxctl-provider-red.log`。
2. `CARGO_NET_OFFLINE=true cargo test --locked -p boox-core --test wechat_login environment_token_blocks_wechat_before_network`：退出 101，在环境令牌生效时仍显示二维码，触发禁止网络/显示的断言。日志 `/tmp/booxctl-wechat-env-red.log`。

修正后：`CARGO_NET_OFFLINE=true cargo test --locked -p booxctl --test auth`，退出 0、14 项通过（`/tmp/booxctl-provider-green.log`）；`CARGO_NET_OFFLINE=true cargo test --locked -p boox-core --test wechat_login`，首批退出 0、4 项通过（`/tmp/booxctl-wechat-green.log`）。后加拒绝、过期、回调失败、空令牌、畸形轮询、显示后取消及 PNG 回归，不把直接通过的新增回归记为 Red。

HTTP 夹具使用绝对读取期限、有限写超时和真实 join；全部账号/令牌为合成数据。继承图片已备份在 `/tmp/booxctl-inherited-qr-fixtures`，仓库图片重新本地生成，不包含实际二维码会话，生成规则见 `crates/core/tests/fixtures/README.md`。

## 线上显示检查

用 Python PTY 启动 `target/debug/booxctl --timeout 20 --verbose auth login --qr`，独立临时配置目录及文件凭据后端，清除继承 token 和测试端点。只在内存观察“等待微信确认”提示，随即发送 SIGINT；未扫码、未执行回调、未读取账号、未保存二维码或原始输出。

结果：`wechat_qr_displayed=true`、退出 130、`credentials_saved=false`。该检查证明默认路径可显示现行线上微信二维码，不证明手机扫码、真实令牌保存、推送或设备收件通过。

## 最终验证

`CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true bash scripts/check_cli.sh`：退出 **0**，**275 Rust + 7 Python** 通过，fmt、严格 clippy、release 构建、依赖边界、无显示 smoke、打包与产物复验均通过。日志 `/tmp/booxctl-wechat-final-check.log`。此前 clippy 的生产代码风格与夹具回调类型告警均已修正，不记为行为 Red。

本轮本机验收已完成；提交推送后的三平台 GitHub Actions 按精确 SHA 另行记录。真实手机扫码、账号认证及设备收件仍未完成。

最终 release 线上显示复验命令：`python3 /tmp/booxctl-wechat-smoke.py target/release/booxctl`。
首次辅助脚本退出 1，产品退出 4 / `socket_error`，未显示二维码、未保存凭据（`/tmp/booxctl-wechat-release-smoke.log`）。原样重试一次后辅助脚本退出 0：显示微信二维码、主动取消退出 130、未保存凭据（`/tmp/booxctl-wechat-release-smoke-retry.log`）。连接失败的根因未确定；重试成功不能证明线上连接永久稳定，也未增加自动登录重试。
