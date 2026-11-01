> **2026-11-01 方向变更（已终止）**：`cli` 分支后续只做纯 CLI，不再接入桌面端（方案 A）。本文档所述桌面切片不再作为本分支待办；未提交的桌面登录会话工作区已撤回并归档在 `/tmp/sandflos-desktop-session-wip`。当前范围与验证见 `tdd-cli-only.md`。以下内容保留为历史记录。

# 桌面登录会话下一片

2026-10-31，base3cc6fb7+七片既有改动，已267Rust7Python全通过。用户允许父commit/push；子agent禁止提交。S2/S3已确认，合成回环/显式临时store，绝不触碰真实用户缓存/keyring/accounts。

目标：实际app由owned会话接收LoginPortal令牌并经显式CredentialStore成功保存后更新内存，失败不能报成功；不自动迁移/删除旧auth_cache.json。业务core/UIapp，复用现有AuthService/store/Portal协议，保留legacy入口。新增公开desktop session模块可测试，提供显式store/paths构造及bounded callback。默认desktop独立路径(不是CLI配置目录)，默认平台keyring，不可用时显示错误、不静默file降级；旧缓存仅兼容读取可保留，不写/转换旧缓存。

App实际start_login使用LoginPortal并持有portal/session；snapshot显式token来自session。剩余旧上传/queue/library仍会读globaltoken：为保持产品可用，允许一个明确临时的**只内存legacy token投影桥**，与app拥有会话同源，禁止调用set_auth_state(会写默认cache)、禁止直接默认cache持久化。可以新增core state helper仅投影内存供旧路径兼容，必须标明未完成全局清零，后续上传迁移删此桥。失败保存不得更新任何token或桥；成功持久化后更新会话及兼容投影。callback错误HTTP不得泄密。

先仔细读app/state.rs、auth_service.rs、auth.rs、state.rs。先报告简短设计给父，再写一个最小可运行新会话adapter保持失败保存仍更新会话的缺陷，并公共行为test观察真实Red后最小Green；明确这是新adaptermissing-feature行为Red不是原baseline。或找到现有公开错误行为先Red。不能以缺API/compile/setup算Red，不rollback现成实现造历史。原始Red/Green命令、退出码、输出保存/tmp/sandflos-desktop-session-red.log/green.log并在tdd-desktop-session.md引用；后续共享机制直接Green。先一个用例再实现，勿先铺全矩阵。

公开回归应覆盖：store保存失败不更新内存、retry成功、显式临时store重建读取、两个store隔离、秘密不Debug/通知输出；真实回环portalcallback成功/失败/duplicate可复用login_delivery公共测试夹具，但不访问上游；如果新session callback可直接供Portal测试用公开接口，无需新private测试。Fixture绝对deadline、write有界、actualjoin。

Own新core desktop_session.rs、core/lib.rs小导出、必要auth_service/state helper、app/state.rs登录初始化/刷新token hunks、新公共测试及TDDdoc；不改services/upload/queue/library协议/manifests。不要app全fmt。portal替换/drop不能阻塞UI20秒，owned session生命周期设计需认真：可以background_executor执行实际join而非spawn detached blocking workers；若做不到先报父，不隐藏风险。writer targeted tests/clippy；父full checks及GitHub macOS check。
