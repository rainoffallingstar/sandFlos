# CLI 认证与配置实施记录

日期：2026-10-29。公共边界为方案 S1/S2/S3；所有测试使用临时配置目录、假客户端或回环 HTTP，不访问真实账号，不使用真实 keyring。

## 已实现的行为

- `AuthService` 显式接收配置目录、HTTP 客户端、凭据存储与环境 token，不读取桌面全局登录态。
- 文件凭据使用 schema 版本、原子替换、Unix 目录 0700 / 文件 0600；Linux 默认没有持久 keyring 时明确报 `credential_store_unavailable`，不会静默退化为明文文件。
- 环境 token 优先于文件凭据，永不落盘；`auth import` 与二维码登录在环境 token 生效时拒绝并报告优先级。
- token 导入拒绝空值、空白、控制字符和超长值。凭据读取失败、损坏版本与写入失败返回稳定 storage 错误；写失败不会创建或替换凭据。
- `auth import` 在本地格式检查后调用用户接口验证凭据，只有验证通过才持久化；401/403 为认证错误且不落盘。`auth status` 默认只读本地状态；`--verify` 才再次调用用户接口，401/403 为认证错误，且不输出服务端响应正文。
- 二维码登录在成功确认后才保存；支持 pending、confirmed、expired、取消与超时，非 TTY 或 `--json` 下由 CLI 拒绝交互式登录。
- HTTP 适配器可注入端点用于回环测试；生产入口仍固定官方服务地址。进程级测试通过仅允许回环地址的 `SANDFLOS_TEST_AUTH_BASE_URL` 注入假服务，验证接口与二维码创建/轮询测试检查方法、路径、查询参数和 Bearer 头。
- `auth status`、导入结果、注销结果不含 token；`ResolvedToken` 的 Debug 也脱敏。

## Red/Green 记录

- 新增 HTTP 适配器测试前，适配器只能访问硬编码线上地址，无法离线验证协议；加入可注入端点后，首轮测试暴露了测试夹具地址错误和请求头大小写断言问题，修正夹具后转绿。这些是测试实现问题，不是产品失败。
- 新增 `verify_without_credentials_requires_login_without_network` 先确认未登录 `--verify` 应返回 3，不发出网络请求；实现已有该行为，测试直接转绿。
- 新增显式配置目录优先级测试首次实现即通过；测试同时确认环境配置目录没有被写入。
- 新增损坏凭据进程测试首次实现即通过，确认退出 5、稳定 code 与 stdout 不泄露文件内容。
- 新增凭据写入失败测试先使用只读目录，被测试进程的目录权限收紧掩盖；改为把配置路径设为普通文件后稳定触发 `credential_write_failed`，并确认未创建 `auth.json`。
- 新增“导入前验证”契约时，现有服务测试立即暴露差异：`auth import` 只做本地格式校验便保存。先在服务层加入 `verify_token` 后保存，并为接受与拒绝两条路径补测试；随后在 CLI 进程层加入回环认证 fixture，验证请求确实为 `GET /api/1/users/me`、携带 Bearer 头，且 401 时退出 3、稳定 code 为 `credential_rejected`、不落盘且不回显服务端正文。
- 验证加入后，`import_then_verify_in_new_process_is_authorized` 的调用记录从一次变为两次（导入一次、`--verify` 一次），按预期更新断言后转绿。这是新契约带来的行为变化，不是测试夹具故障。

## Green 结果

- `cargo test --locked -p boox-core auth_service --lib`：23 passed，0 failed（含导入前验证与拒绝不落盘）。
- `cargo test --locked -p boox-core -p sandflos-cli`：core 80 unit + 8 library integration passed；CLI 13 auth + 7 process + 8 push/queue passed；0 failed。
- `cargo clippy --locked -p boox-core -p sandflos-cli --all-targets -- -D warnings`：通过。
- `cargo fmt -p boox-core -p sandflos-cli -- --check`：通过。
- `git diff --check`：通过。

## 跨平台与真实服务未验证

- 未调用真实 BOOX 登录、二维码、用户接口或 keyring。
- 未在 macOS Keychain、Windows Credential Manager 或 Windows ACL 上做运行验收。
- 终端二维码可扫码性需要人工 smoke；自动化仅验证非 TTY 拒绝与服务层生命周期。
