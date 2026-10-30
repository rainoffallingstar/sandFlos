# CLI 书库实施记录

日期：2026-10-29。公共边界为方案 S2/S3，SQLite 使用临时真实数据库，WebDAV 使用回环 HTTP。

- 接手时已有 Calibre 配置测试和占位列表实现。全套基线运行：core 66 项通过；`selected_library_and_format_resolve_the_expected_book` 因列表长度 0（预期 1）失败。补齐显式目录的只读查询和格式定位后该测试通过。
- `explicit_zotero_directory_resolves_local_attachment` 先因配置返回 None（预期显式数据目录）失败；随后实现配置与安全附件解析。
- `zotero_list_reads_explicit_database_and_filters_titles` 先因列表为空失败；补齐只读列表后独立重跑通过。
- `missing_local_attachment_downloads_webdav_and_cleans_up` 先因 WebDAV 占位返回 not implemented 失败；实现受限 ZIP 下载、凭据保存与 RAII 临时文件清理。

新增实现不读取桌面端全局配置，不修改源 SQLite。Calibre 文件及 Zotero 本地存储附件限制在配置书库内；WebDAV 禁止跨主机重定向，非回环地址要求 HTTPS，限制压缩与解压大小为 200 MiB，拒绝越界 ZIP 路径和符号链接。

完整 Green 结果及跨平台未验证事项以 tdd-integration.md 为准。测试过程中与其他模块存在共享编译锁及暂时编译错误；这些不算业务 Red。

回归：`cargo test --locked -p boox-core --test cli_libraries`，8 passed、0 failed（包含正常 WebDAV 下载与 ZIP 越界拒绝）。配置拒绝无关 SQLite 的测试先因成功接受空数据库而失败，加入必要表校验后通过。配置修改持有 OS 文件锁，避免并发 read-modify-write 覆盖其他书库设置。

集成审查补充：书库错误改为 `LibraryError`，包含类型化 kind 与脱敏 message；WebDAV 401/403 映射认证错误，连接/响应读取映射网络错误。随后补充 401 回归测试（这是审查后的回归测试，不宣称为先 Red 后 Green）。ZIP 越界测试也是已有防护的回归验证。

后续审查补充（2026-10-29）：新增 `webdav_rejects_archive_without_requested_attachment`，回环服务返回只有 `unrelated.pdf` 的 ZIP，首次运行失败（`archive should fail`），确认旧实现会把其他文件作为目标附件。移除任意首文件回退，仅接受 SQLite 指定的附件相对路径；独立回归 `cargo test --locked -p boox-core --test cli_libraries`：10 passed、0 failed（含密码修复）。

同轮新增 `webdav_password_can_contain_spaces`，先因 `WebDAV credentials unavailable` 失败：复用 token 凭据存储会拒绝包含空格的有效 WebDAV 密码。新配置将密码做 Base64 编码后交给凭据后端，配置记录编码标记，下载时解码；Base64 仅做格式适配，存储保护仍由原凭据后端提供。旧无编码标记配置保持可读。回环请求断言实际 Basic Authentication 的字面值以验证密码未被修改。

最终审查补齐（2026-10-30）：T11 此前分别证明下载与本地附件推送，缺少组合进程验收。`crates/cli/tests/library_push_e2e.rs` 新增真实 CLI 配置书库、stdin 保存 WebDAV 凭据、本地缺失附件下载后推送的完整链路；用 ZIP 中的字面附件内容核对 OSS 请求体。成功退出 0；最终 `saveAndPush` 失败退出 6、保留 unknown，且仅提交一次。两条链路均根据公开 JSON 的附件 path 确认文件及临时父目录已删除，源 SQLite 字节未改变。独立执行 `cargo test --locked -p sandflos-cli --test library_push_e2e`：4 passed、0 failed。这是已实现功能的新增组合回归，不宣称为新的业务 Red/Green；协调 agent 修正了测试初稿中的失败阶段和清理断言。

边界补齐（2026-10-30）：新增三项 `crates/core/tests/cli_libraries.rs` 回归。

- `webdav_rejects_archive_symlink_entries`：回环 ZIP 只包含一个指向归档外的符号链接条目，要求 `unsafe ZIP path` 且错误类型为 LocalIo。
- `calibre_listing_is_paged_globally_across_libraries`：两个书库各两本书，验证先合并再排序，`offset`/`limit` 跨库全局生效，越界页为空。
- `calibre_listing_reports_a_locked_database_without_hanging_and_can_be_retried`：另一连接 `BEGIN EXCLUSIVE` 时，列表在 2 秒内返回 `cannot query Calibre database`，回滚后可重试成功。Red 观察：默认 SQLite busy timeout 使首次断言超时；`list_books_at` 设置 500 ms busy timeout 后 Green。这是本轮实际 Red/Green 的库边界修复。

独立执行 `CARGO_NET_OFFLINE=true cargo test --locked -p boox-core --test cli_libraries`：17 passed、0 failed。前两项是已有实现的新增回归，不冒充先 Red 后 Green。迁移侧另补 `migration_validates_credentials_preserves_source_and_refuses_overwrite` 的新进程 `auth status --verify` 断言，确认导入后的 token 可由后续进程实际使用；`cargo test --locked -p sandflos-cli --test migration`：3 passed、0 failed。
