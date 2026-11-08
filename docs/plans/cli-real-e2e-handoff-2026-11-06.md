# 真实账号与设备端到端验收交班（2026-11-06）

## 当前结论

代码整改及自动化验收完成并已推送。真实端到端测试**尚未执行**：当前环境没有已登录账号，用户尚未提供指定测试设备的连接信息。不能将云端自动化 fixture 或 CI 通过当作真实设备验收。

本轮用户明确授权真实账号、单台真实设备的端到端测试；此前“不使用真实账号/设备”的测试限制在此指定验收范围内被覆盖。只使用合成测试文件，不读取私人书籍；本轮不涉及版本标签或发布。不要把 token、密码、二维码认证内容、完整私人队列写入 Git 或聊天。

## 已执行与证据

- 起始 HEAD / origin/cli：`d13e75a4de93ce6ab9937e63d7688abd3ddb068c`；工作树干净。`git push origin cli` 退出0，`Everything up-to-date`。
- 产品代码：`76a42f01a0f9a9ad8ffd5c261c55ef0bec23c211`。`d13e75a` 只补文档并跳过CI，不能声称它有独立构建。
- 2026-11-06 重新查询：CLI run `41875934420` success，三平台构建及下载复验6/6；release rehearsal `41875934417` success，package3/3，draft-release按分支演练设计跳过。历史失败已经修正或重跑，不是当前错误。
- 真实环境离线 `target/release/booxctl --json auth status`：退出0，`authorized:false`，`source:none`。已去除测试端点环境变量；未请求真实云端。
- 当前进程无 BOOXCTL_TOKEN、无自定义配置目录/凭据后端；PATH 中无 adb。未扫描局域网或尝试未知 SSH 主机。
- 测试拟使用已从 GitHub 下载的 `/tmp/booxctl-76a42f0-linux/booxctl`；`python3 scripts/verify_cli_package.py /tmp/booxctl-76a42f0-linux` 退出0，输出 `Verified CLI package: booxctl`。
- 合成 EPUB 已准备：`/tmp/booxctl-e2e-20261106/booxctl-e2e-5e3ff0ef.epub`，1894 bytes，SHA-256 `9c524e10f67604e6c04a2ac9232341d93d2d952ba17138af896a214050ca4fbe`。标题 `booxctl E2E 5e3ff0ef`，收件码 `5e3ff0ef-e8a7-457a-a598-8931491864f0`。ZIP CRC检查通过；未宣称已由真实阅读器打开。
- `/tmp` 材料可能被清理；使用前重新核对存在性及哈希，缺失则生成新的合成文件并记录新标识。

## 等待用户提供（已在会话中询问）

1. 指定设备及连接方法：ADB、SSH、局域网地址，或用户手动操作；同时记录机型/固件和是否已登录目标账号。不猜测设备身份或远程登录配置。
2. 登录方法：已有测试配置目录、本机受保护的凭据文件路径，或用户在交互终端扫码。不要在聊天中发送秘密；不擅自复用无关软件中的账号。
3. 若设备无法远程检查，需用户在设备上打开测试文件并核对收件码。云端 accepted 只能证明服务接受推送。

## 材料到位后的执行顺序

- 选择独立配置目录。Linux 持久化需显式文件后端；目录0700、凭据0600，保留在 Git 外。使用已有配置时不覆盖原凭据。
- 从 stdin 导入指定凭据或执行交互 `auth login --qr`；记录脱敏结果。执行 `--json auth status --verify`，成功后再操作云端。
- 查询 `devices list`、`storage status`，确认指定设备与测试账号关系；仅保存必要的脱敏摘要。
- 对上述合成 EPUB 执行一次 `push`（单请求超时及进程总截止均有界）；记录退出码、阶段、消息 ID 的脱敏引用。遇到 unknown 先查询队列，不自动重复上传。
- `queue list` 只核对本次文件/消息；不复制其他业务记录到报告。设备收到后核对文件名、可读性和页面收件码，条件允许时再比对字节哈希。
- 将“认证有效”“云端接受”“队列可见”“设备收到”“设备可打开且收件码匹配”分别记录。没有设备证据时维持部分完成。
- 不自动删除既有云端记录或本地凭据；本次测试文件的清理单独记录。重推、队列删除、WebDAV/书库等扩展操作只针对明确的测试材料；缺少相关服务不伪造通过。

## 结果表

| 验收项 | 状态 | 证据/阻塞 |
| --- | --- | --- |
| Git提交推送 | 通过 | 起始提交已同步；本记录另行文档提交 |
| GitHub Actions | 通过 | 上述精确产品SHA，6/6与3/3 |
| 真实账号认证 | 待执行 | 本机未登录，待登录材料 |
| 指定真实设备连接 | 待执行 | 尚无设备连接信息 |
| 真实BOOX/OSS推送 | 待执行 | 依赖账号认证及设备确认 |
| 队列可见 | 待执行 | 未推送测试文件 |
| 设备收到并打开 | 待执行 | 需远程设备访问或用户实机确认 |
| WebDAV、系统钥匙串、Windows Ctrl-C | 未验收 | 不由这台设备单次推送自动覆盖 |
| tag→draft正式发布 | 未执行 | 不在本次授权范围 |

下一位执行者先检查本会话是否已有用户回复，复用已准备材料和已通过CI，不重复代码整改。完成后把本表更新为真实结果，再形成最终通过或失败交班。

## 默认微信登录更新

本机微信默认登录变更与验证记录见 [微信切片](cli-wechat-default-2026-11-06.md)。此前下载的 `/tmp/booxctl-76a42f0-linux/booxctl` 仍为 BOOX 助手扫码旧版；本次人工验收须使用工作区新构建的 `target/release/booxctl`。

```bash
export BOOXCTL_CONFIG_DIR="$HOME/.local/share/booxctl-real-e2e"
export BOOXCTL_CREDENTIAL_STORE=file
BOOXCTL_BIN=/home/fallingstar10/shire/sandFlos/target/release/booxctl
unset BOOXCTL_TOKEN BOOXCTL_TEST_AUTH_BASE_URL BOOXCTL_TEST_API_BASE_URL BOOXCTL_TEST_OSS_BASE_URL
"$BOOXCTL_BIN" auth login --qr
# 微信扫一扫，手机确认后，等待 authorized=true
"$BOOXCTL_BIN" --json auth status --verify
"$BOOXCTL_BIN" devices list
```

截至本次自动执行，只有线上微信二维码显示与主动取消通过；手机扫码、账号认证成功、合成 EPUB 推送及设备收到/打开仍待用户配合。不要把旧 SHA 的 GitHub Actions 结果视作当前微信改动的远端验收结果。

## 2026-11-07 跟进

微信改动已推送产品 SHA `2f0fe1fb4a2f1e86f51a21f53ede6a442c73f032`。打包演练 run `42239177349` 三平台成功；CLI run `42239177826` 首轮 5 项成功，macOS 下载复验因托管 ARM64 runner 容量不足未取得执行器而取消，已通过 `gh run rerun 42239177826 --failed` 请求重跑。当前尚不能标记新 SHA 的 CLI 6/6 通过。

真实测试目录仍未登录；用户在远程无头服务器使用，正在确认是否需要浏览器授权链接入口。现有 SSH 交互二维码可用，CLI 浏览器链接入口尚未实现。未执行真实云端推送或设备验收。

最终补记：上述 CLI run 的 attempt 2 已 **success，6/6**，新产品 SHA 的三平台构建及下载复验全部通过；打包演练亦 3/3 通过。远端自动化阻塞已解除，剩余阻塞仅真实账号登录与设备验收。

## 2026-11-08 真实登录与推送

用户使用 `897a711` 构建的 `target/release/booxctl auth login --url`，在本地浏览器完成微信授权；CLI 输出 `authorized=true, source=file`，随后 `auth status --verify` 返回 `authorized:true`、退出 0。凭据保存在用户 shell 的 `$HOME/.local/share/booxctl/auth.json`（该 shell 为 root）；为在 `fallingstar10` 执行会话中复用，用户将文件复制到 Git 忽略的 `target/private-e2e/auth.json`（目录 0700、文件 0600）。

- `devices list`：退出 0，单台在线设备 `Leaf5C`，ID `1235920`，MAC `00:0c:c1:55:be:a6`；`transfer_host` 为 null，未声称局域网设备服务可达。
- 合成 EPUB 复验：存在，1894 bytes，SHA-256 `9c524e10f67604e6c04a2ac9232341d93d2d952ba17138af896a214050ca4fbe`，与准备记录一致。
- 推送尝试 1（默认 20 秒）：OSS 上传阶段 `network_error`，退出 4，未 accepted。
- 推送尝试 2：OSS 上传成功，`write_message` 收到 HTTP 502，结果 `unknown`，退出 6，本地无消息 ID。
- 推送尝试 3（`--timeout 30`）：OSS 上传阶段失败，退出 4，文档计数仍为 0。
- 推送尝试 4（`--timeout 60`）：**accepted**，退出 0，消息 ID `17069e0d-723e-4090-9abe-d5ac468e82fb`，revision `1-d814cc26c318a677a933d17c7916a9db`，resource `69d338d37547a702f74efbfc/push/a7cad5a7-4e62-4871-92df-ae879d42d0c0.epub`。
- 队列可见性：`queue list` 多次在 neocloud `_changes` 超时或 HTTP 502；同步令牌、`users/me`、bucket 和 STS 接口正常。直接读取已接受消息文档同样返回 502。因此“队列可见”仍未独立确认。尝试 2 的 `unknown` 发生在消息写入响应丢失时，未能核对是否留下重复记录；文档计数查询在此账号返回 0，不能作为证据。
- 账号查询 `totalDocCount=0`、`storage_used=3385595732`；文档计数未随 accepted 更新，可能为延迟或上游故障，不能据此推断收件状态。

仍需用户在 `Leaf5C` 上确认是否收到 `booxctl-e2e-5e3ff0ef.epub` 并能打开；云端 accepted 不等于设备收件。队列故障恢复后应只针对上述测试文件名和消息 ID 复核重复记录，不重推。

| 验收项 | 状态 | 证据/阻塞 |
| --- | --- | --- |
| 真实账号认证（链接模式） | 通过 | `897a711`，`auth status --verify` authorized |
| 指定真实设备在线 | 通过 | Leaf5C，ID 1235920 |
| 真实BOOX/OSS推送 | 通过（云端接受） | 消息 `17069e0d-…`，退出 0 |
| 队列可见 | 阻塞 | neocloud `_changes` 超时/502 |
| 设备收到并打开 | 待确认 | 需用户在 Leaf5C 上核对文件名 |
| 重复记录核对 | 阻塞 | 一次写入结果 unknown，队列不可用 |
