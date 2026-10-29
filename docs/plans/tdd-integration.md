# CLI 集成验证记录

用户已在 2026-10-29 授权按方案实施，S1–S4 作为本次公共测试边界。三个实施 agent 使用 deepseek-v4.1-flash / high；主 agent 负责集成和审查。

## 桌面依赖隔离

将 core 的 autostart 模块及 auto-launch 依赖移至 app，调整桌面调用路径。该变动是保持行为的依赖迁移，以原有测试回归及依赖图验收，不宣称为新功能的 Red/Green。

- `cargo test -p boox-core --offline`：39 passed，0 failed。
- keyring 为 Windows 增加 windows-native；Linux 缺少持久后端时必须明确报不可用，不能使用默认 mock 后端冒充已保存。
- 现有 Linux warning：`default_zotero_root` dead_code；桌面构建交给 macOS CI 验证。

具体业务切片的 Red/Green 记录分别见 tdd-cli.md、tdd-auth.md、tdd-services.md（对应实施完成后写入）。
