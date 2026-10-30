# 阅读日期与凭据失败边界

2026-10-30，沿用已确认的 S1/S3。

## T9 阅读日期

`CARGO_NET_OFFLINE=true cargo test --locked -p sandflos-cli --test reading_calendar`：新增真实 CLI 进程验收，以两个不同时区和跨年日期验证请求参数原样传递，并要求输出显式标记未知时区。Red 为 `data.get("timezone")` 返回 `None` 而非 `Some(Null)`；最小实现给 `ReadingStats` 加 `timezone: Option<String>`，HTTP 适配器在上游没有时区信息时返回 `None`，文本输出也明确 unspecified。Green：1 项通过。记录 `/tmp/sandflos-reading-calendar.log`、`/tmp/sandflos-reading-calendar-green.log`。

另加上游三个统计端点均返回 `data:null` 的组合回归，断言三个输出字段都保留 null；属于已有行为的新增验收，不声称修复前失败。服务器不可用等失败仍沿用类型化 HTTP 分类，不能将错误伪装成空统计。

## T2/T3 凭据持久化

`crates/cli/tests/auth_storage_failures.rs` 在真实 CLI 进程边界补回归，不新增生产实现、不伪造 Red 历史：

- 已有凭据且存储锁被占用时，导入新 token 退出 5；旧文件内容不变，释放锁后新进程 `auth status --verify` 使用原 token 验证成功。首项已直接 Green。
- `auth.json` 被非空目录占位时，logout 删除失败退出 5，不清除哨兵；修复路径后可重试。
- Windows 专属：用真实文件句柄禁用 `FILE_SHARE_DELETE` 阻止 `MoveFileExW`，要求保存失败保留旧凭据、释放句柄后仍可验证并再次导入。这项须由 Windows CI 验收，Linux 不将其记为已执行。

锁冲突覆盖写入开始前的失败，不能单独证明所有写入中断或磁盘故障场景。Windows 文件替换测试覆盖实际替换失败；平台 keyring/ACL 与真实权限环境 smoke 仍单列人工验收。

Linux 组合回归：`CARGO_NET_OFFLINE=true cargo test --locked -p sandflos-cli --test auth_storage_failures --test reading_calendar`，4 项通过，0 失败（Windows 专属用例未在 Linux 编译执行）。记录 `/tmp/sandflos-calendar-storage-regression.log`。

## T4 二维码失败路径

新增 `crates/core/tests/auth_qr_failures.rs`，经真实 HTTP 适配器与公开 `AuthService` 验证，使用临时凭据目录；属于已有实现的边界回归，全部直接 Green，没有新增生产实现：

- `status:1` 缺少 token：认证失败，不落盘。
- HTTP 403 拒绝、连接直接断开：分别认证/网络分类，响应正文不进入错误输出。
- 等待预算为零：创建二维码后不发起轮询；pending 后取消不发起下一次轮询。
- HTTP 轮询超过客户端 1 秒超时：明确 Timeout / 退出 4，耗时有上界且不落盘。

`CARGO_NET_OFFLINE=true cargo test --locked -p boox-core --test auth_qr_failures`：3 项通过、0 失败，约 2 秒；记录 `/tmp/sandflos-qr-boundaries-final.log`。中间并行改动曾移除正在使用的 fixture 字段导致编译冲突，恢复后重跑通过；编译冲突不计作 TDD Red。回环 listener、读写与服务等待均有有限边界，fixture 读取完整 HTTP 请求体后响应。

真实终端展示、手机可扫码、平台凭据写入和生产扫码响应仍未由这些合成 HTTP 验收证明。
