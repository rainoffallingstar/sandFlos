# CLI-only 方向：清理与 TDD 记录

2026-10-31 至 2026-11-01。用户先选择方案 A（停止桌面投入），随后升级为**方案 B：从 `cli` 分支删除桌面代码**。基线 `d3810d0fb5d9646bd18b49da8535a0c6c5cd52bc`（= `origin/cli`），无真实账号、凭据、书库，未创建 tag 或 release。

## 方向决定

- `cli` 分支的产品范围收敛为 `crates/core` + `crates/cli`、`sandflos` 二进制及其发布产物。
- 方案 B 执行结果：`crates/app`（GPUI）、`src-tauri`、`dist/`、`assets/`、
  `release-macos.yml`、`scripts/build_dmg_tauri2.sh` 与
  `scripts/internal_release_check.sh` 已从 `cli` 分支删除，工作区成员只剩 core 与 cli。
- 桌面实现的完整历史仍保存在 `main` 分支（git 历史未改写），需要时可从那里取回。
- `crates/core` 中 CLI 仍在使用的模块（`api`、`auth*`、`push`、`device`、
  `dashboard`、`zotero`、`calibre`、`cli_libraries`、`persistence`、
  `cancellable_*`、`state`、`diagnostics`）全部保留，不按“桌面代码”删除。
- 未完成的桌面迁移计划（app-owned 登录会话、上传/队列迁移、桌面人工回归）不再作为 cli 分支的待办。

## 实际改动

方案 A（提交 `0113905`）：

1. `.github/workflows/cli.yml`：移除 `desktop-compatibility` job（原为 `cargo check --locked -p send2boox-desktop-gpui`）。其余 cli / verify-package 三个平台 job 不变。
2. `README.md`：从“GPUI 桌面控制中心”改写为 CLI 产品说明；去掉桌面构建、托盘、窗口、`src-tauri`、`crates/app` 宣传。
3. `docs/cli.md`：去掉桌面构建指引、桌面 CI 说明与桌面配置互操作措辞，明确本分支只构建、测试和发布 CLI。
4. `docs/plans/cli-github-status.md`：记录方向变更与归档位置。
5. 撤回上一片未提交的桌面登录会话工作区（见下）。

方案 B（本片）：

1. `git rm -r`：`crates/app`(17 文件)、`src-tauri`(32)、`dist`(4)、`assets`(2)、
   `.github/workflows/release-macos.yml`(1)、`scripts/build_dmg_tauri2.sh`(1)、
   `scripts/internal_release_check.sh`(1)。
2. `Cargo.toml`：members/default-members 只剩 `crates/core`、`crates/cli`；移除
   `exclude = ["src-tauri", "dist"]`。
3. `Cargo.lock`：由 8958 行剪到 2252 行，删除 536 个桌面依赖包（gpui、
   gpui-component、tauri、rfd、tray-icon、auto-launch 等）。
4. 删除 `crates/core/tests/desktop_refresh.rs`（12 项桌面刷新契约测试）。
5. `.gitignore`：去掉 `src-tauri/target/`、`.build-cache.noindex/`、`*.app`。
6. `README.md` / `docs/cli.md`：去掉“仓库仍保留桌面包”的措辞。

### 有意保留的 core 桌面兼容入口

`crates/core/src/dashboard.rs`（`DashboardRefreshInput`、严格刷新）、
`push.rs` 的桌面包装（`perform_native_uploads`、`dashboard_push_resend` 等）与
`state.rs` 的全局缓存仍在 core 中。它们不引入桌面依赖，CLI 依赖闭包也不包含
GPUI/Tauri；删除它们属于单独的重构，不在本片“移除桌面端”的范围内。

## 撤回桌面会话工作区

该片（`desktop_session.rs`、`tests/desktop_session.rs`、`tdd-desktop-session.md`、三份桌面审查文档、`app/state.rs` 登录 hunks）已完成 core 级实现与 5 项公共回环测试通过，但因方向变更不再进入 cli 分支。撤回前已完整归档到工作区外：

- `/tmp/sandflos-desktop-session-wip/tracked.patch`：所有已跟踪文件的 `git diff --binary`
- `/tmp/sandflos-desktop-session-wip/files/...`：未跟踪文件逐字副本
- `/tmp/sandflos-desktop-session-wip/README.txt`：快照时间、基线 SHA、当时的测试结论

已跟踪改动用 `git checkout --` 还原，未跟踪文件从工作树移除；归档未删除。该片从未提交或推送。

## 保留的非桌面改动

`crates/core/tests/login_delivery.rs` 的 HTTP 夹具（仅测试代码）改为绝对总 deadline、有界写超时、逐次 read 的剩余时间超时，属于测试健壮性改进，与桌面无关，保留。

该夹具最初来自桌面会话片的审查修订，但改动只影响核心登录回环测试的读取方式，不引入任何桌面依赖或 app API。保留理由：原夹具的 `read_to_string` 只有单次 2 秒读超时，没有总截止，慢回环可能无限拉长测试。

## 验证

```text
CARGO_NET_OFFLINE=true bash scripts/check_cli.sh
```

退出码 `0`：**267 项 Rust、7 项 Python，0 失败**，fmt、严格 clippy、release 构建、依赖边界、无显示 smoke、打包与产物校验全部通过。日志 `/tmp/sandflos-cli-post-cleanup-check.log`。

定向复验（清理后）：

```text
cargo test --offline -p boox-core --test login_delivery --test credential_lock
```

退出码 `0`：6 项 login-delivery + 1 项 credential-lock。日志 `/tmp/sandflos-login-after-cleanup.log`。

README / docs / CI 文本修订完成后，重跑同一命令（`/tmp/sandflos-cli-only-final-check.log`）仍退出 `0`：267 项 Rust、7 项 Python，0 失败，fmt、严格 clippy、release、依赖边界、无显示 smoke、打包与产物校验全通过。文本与 YAML 改动不改变 Rust/Python 测试内容，但统一检查确实在最终工作树上通过。

`cli.yml` 结构确认只余 `cli` 与 `verify-package` 两个 job；仓库内已无 `desktop-compatibility` 或 `send2boox-desktop-gpui` 的 CI 引用。

## 限制

- 未能对本机做桌面类型检查（缺 GTK3 开发包与部分 crate 缓存），这与本方向无关：桌面已不在 cli 分支的验证范围。
- 远端 CI 对当前提交尚无 run；本机通过不等于远端三个平台通过。
- 真实凭据、扫码、BOOX/OSS/WebDAV、设备接收与 tag→draft release 仍未验收。
- `crates/core` 中 `dashboard`、`device`、`auth_service` 仍被 CLI 使用，不能按“桌面代码”删除。真正只服务桌面的入口（如 `build_dashboard_snapshot` 的全局兼容包装）留待后续按 CLI 需求单独收口。

## CI 失败与测试加固

首次推送 `0113905` 的 CI run `41255922160`：ubuntu 与 windows 通过，macOS 在
`transfer_streaming::slow_reader_produces_multiple_distinct_read_samples_before_completion`
失败——`ServiceError { kind: Network, message: "网络请求超时" }`。

原因是既有测试的时序余量不足，不是本片改动：该用例让回环服务器按 64 KiB / 10 ms
接收 16 MiB，同时上传请求超时写死 10 秒。慢 runner 上接收延迟本身就要 ~2.6 秒，
叠加 reqwest/CI 调度即可越过 10 秒，被误判成网络失败。本机连续 5 次均通过
（约 3.7–4.1 秒），说明它只在受载环境暴露。

加固方式（仅测试）：payload 降到 4 MiB、服务器延迟改为 5 ms（约 64 次暂停、
~0.3 秒延迟），请求超时放宽到 30 秒。断言不变——仍要求 ≥2 个严格递增的读取
样本、样本不超过文件大小、最后样本等于文件大小、单次 PUT。改后本机连续 3 次
全通过，整个测试文件从约 4 秒降到约 0.5 秒。

复验：`CARGO_NET_OFFLINE=true bash scripts/check_cli.sh` 退出 `0`，
267 项 Rust + 7 项 Python，fmt/clippy/release/依赖边界/无显示/打包/产物校验
全通过，日志 `/tmp/sandflos-cli-only-flakefix-check.log`。

## 方案 B 验证

删除桌面代码后重跑同一命令：退出 `0`，**255 项 Rust**（原 267 减去被删的
12 项 `desktop_refresh` 契约测试）+ 7 项 Python，fmt/clippy/release/依赖边界/
无显示/打包/产物校验全通过，日志 `/tmp/sandflos-cli-only-planb-check.log`。

- `cargo tree -p sandflos-cli -e normal` 中已无 `gpui`、`gpui-component`、
  `tauri`、`rfd`、`tray-icon`、`auto-launch`。
- `Cargo.lock`：8958 → 2252 行，删除 536 个包，无新增包；`dirs`/`digest`/
  `block-buffer`/`crypto-common`/`rand_chacha`/`redox_users`/`dirs-sys` 回到
  CLI 实际约束的版本（此前被桌面依赖抬高）。
- 远端 CI run `41258397146`：6/6 success（三个 CLI + 三个下载产物复验）。
- 桌面实现仍可从 `main` 分支历史取回；本分支不保留副本。

## 已知限制（方案 B 后）

- `crates/core` 仍含桌面时代的兼容包装（`dashboard` 严格刷新、
  `push::perform_native_uploads` / `dashboard_push_resend` / `dashboard_push_delete`、
  `state` 全局缓存）。它们不引入桌面依赖，属后续单独重构，不在本片范围。
- 真实平台验收仍未执行：系统凭据存储、扫码登录、BOOX/OSS/WebDAV、
  设备接收、tag→draft release。

## 关于 OSS 签名的澄清

清理过程中一度怀疑 `HttpPushBackend::upload_bytes` 的 OSS `string_to_sign` 漏了
canonical resource。逐字符核对后确认签名串是
`PUT\n\n{content_type}\n{date}\n{token_header}\n{canonical_resource}`，
与 `push.rs` 的旧实现一致；`transfer_streaming.rs` 的独立 HMAC 期望也以
`/fixture/fixture/book.pdf` 结尾。**生产签名未发现问题，无代码改动。**
（此前看到“缺失”是终端对包含 token 字样的字符串做密钥脱敏造成的显示假象。）
