# 书库与迁移边界补验

日期：2026-10-30。S2/S3 公共边界：Calibre/Zotero 使用临时真实 SQLite，WebDAV 使用回环 HTTP，迁移使用真实 CLI 子进程。起点为 `4d6a48a`；本文件只记录本轮 boundary_finish 范围内的工作。

## 实际 Red/Green

1. **锁定 SQLite 的提示失败**。新增 `calibre_listing_reports_a_locked_database_without_hanging_and_can_be_retried`：另一连接执行 `BEGIN EXCLUSIVE` 后，列表须在 2 秒内返回 typed `LocalIo`，回滚后重试成功。Red：SQLite 默认 busy timeout 使首次断言超时；最小修复在 `list_books_at` 设置 500 ms busy timeout，Green。

其余用例为已有防护或实现的直接回归，不冒充当轮先行 Red。

## 直接 Green 的边界回归

- `calibre_listing_is_paged_globally_across_libraries`：两个库合并后再排序，`offset`/`limit` 全局生效，越界页为空；标题/作者搜索与搜索后再分页正确。断言两个源 `metadata.db` 字节不变。
- `calibre_corruption_after_configuration_is_reported_without_modifying_the_source`：配置成功后将 `metadata.db` 替换为损坏内容，列表返回 `cannot query Calibre database`，源字节不变。
- `configuration_rejects_an_unrelated_sqlite_database` 扩充：空 SQLite 与损坏 `metadata.db` 都在配置阶段被拒绝，损坏源字节不变。
- `unreadable_webdav_credentials_fail_before_opening_a_connection`：凭据文件损坏，以及凭据路径被目录占位两种情况下，均在发起 WebDAV 连接前返回 `LocalIo` / `WebDAV credentials unavailable`；回环 listener 无连接。
- `webdav_rejects_archive_symlink_entries`：ZIP 只含指向归档外的符号链接时拒绝为 `unsafe ZIP path`。
- corrupt ZIP 用例改名为 `webdav_corrupt_archive_is_a_typed_error`，删除与响应正文无关的占位秘密断言，改为精确断言 `invalid WebDAV ZIP`。
- `migration_validates_credentials_preserves_source_and_refuses_overwrite`：迁移后由新的 `auth status --verify` 进程使用导入 token 成功验证；重复迁移仍被拒绝，源文件不变。
- `corrupted_migration_source_fails_without_creating_target_and_can_be_retried`：损坏源不创建目标、不发认证请求；修正源后重试成功，并且**再次由新进程** `auth status --verify` 使用 `fixture-token` 成功，源文件逐字节不变。
- `migration_source_without_a_token_is_rejected_before_persisting`：缺 token 在持久化前以稳定代码拒绝，不发请求。

新增回环 listener 均使用非阻塞 accept 加截止时间，连接切回阻塞并设置读写超时；新增 fixture 线程通过 `join_bounded` 在 8 秒上限内 join，避免测试挂起。

## 验证

```text
CARGO_NET_OFFLINE=true cargo test --locked -p boox-core --test cli_libraries
19 passed; 0 failed

CARGO_NET_OFFLINE=true cargo test --locked -p sandflos-cli --test migration
3 passed; 0 failed
```

两项均使用 `--locked`，无真实账号、WebDAV 或平台凭据访问。

## 父 agent 审查补修

上述 19 项是子 agent 交付时的目标结果。父 agent 进一步发现 `books(id)` 的无关数据库仍能通过配置校验，新增 `calibre_configuration_rejects_a_books_table_without_the_required_schema` 得到实际 Red（配置返回 Ok）。最小修复校验书籍列表与格式选择实际需要的 books/data 列；将原隔离测试中的简化成功夹具补成兼容结构后 Green。书库集成目标现为 20 项，全部通过；源数据库字节保持不变。日志 `/tmp/sandflos-calibre-schema-{red,green}.log`。

五处 WebDAV fixture 复用同一有限 accept/读写设置，包含损坏 ZIP、404 和展开超限用例；父 agent 补上了只限制 join、却仍阻塞于 accept 的缺口。最终统一验收结果见交班记录。

## 未验证边界

- 未使用真实 Calibre 更新中的锁、网络文件系统、权限拒绝或磁盘故障样本；500 ms 超时是明确的失败界限，不是通用性能结论。
- 未连接真实 WebDAV 服务；回环夹具不能证明服务端认证/分块/重定向行为。
- 未验证 macOS Keychain、Windows Credential Manager 或 Windows ACL。
- ZIP 的压缩炸弹、超大响应和损坏条目已有覆盖，但真实 200 MiB 吞吐与内存峰值仍需人工验收。
