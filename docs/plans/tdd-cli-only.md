# CLI-only 方向：清理与 TDD 记录

2026-10-31 至 2026-11-01。用户决定：`cli` 分支后续只做纯 CLI，不再接入桌面端（方案 A：停止桌面投入，不从分支删除桌面包）。基线 `d3810d0fb5d9646bd18b49da8535a0c6c5cd52bc`（= `origin/cli`），无真实账号、凭据、书库，未创建 tag 或 release。

## 方向决定

- `cli` 分支的产品范围收敛为 `crates/core` + `crates/cli`、`sandflos` 二进制及其发布产物。
- `crates/app`（GPUI）、`src-tauri`、`dist/`、桌面宣传资产、`release-macos.yml` 与桌面脚本保留在仓库中用于历史兼容和 `main` 分支，但**不属于 cli 分支的构建、测试、CI 或发布目标**。本片不删除它们：删除会与 `main` 产生大差异，也可能误删 CLI 仍在使用的 core 模块（如 `device`、`dashboard`）。
- 未完成的桌面迁移计划（app-owned 登录会话、上传/队列迁移、桌面人工回归）不再作为 cli 分支的待办。

## 实际改动

1. `.github/workflows/cli.yml`：移除 `desktop-compatibility` job（原为 `cargo check --locked -p send2boox-desktop-gpui`）。其余 cli / verify-package 三个平台 job 不变。
2. `README.md`：从“GPUI 桌面控制中心”改写为 CLI 产品说明；去掉桌面构建、托盘、窗口、`src-tauri`、`crates/app` 宣传。
3. `docs/cli.md`：去掉桌面构建指引、桌面 CI 说明与桌面配置互操作措辞，明确本分支只构建、测试和发布 CLI。
4. `docs/plans/cli-github-status.md`：记录方向变更与归档位置。
5. 撤回上一片未提交的桌面登录会话工作区（见下）。

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
