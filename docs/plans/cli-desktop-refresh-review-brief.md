# 桌面刷新双轴审查

固定点3cc6fb7加working/untracked，commit列表为空，仅本次desktop refresh增量。git diff 3cc6fb7 -- crates/core/src/dashboard.rs crates/core/src/device.rs crates/core/src/api.rs crates/app/src/state.rs crates/app/src/views/push.rs 并读新core/tests/desktop_refresh.rs及TDD。/tmp/sandflos-dashboard-review-base保存开始前dashboard/device/api/lib及app state源码。前6片255Rust7Python已通过，不重复审查旧片。

规范源AGENTS共同约束、README业务放core/UI放app；方案源cli-desktop-refresh-brief.md，上层cli-state-migration与cli-review-and-tdd S2/S3。无生产编辑/测试/commit/push；最多6读/tool调用后写<=400字报告即结束。

Standards写cli-desktop-refresh-standards-review.md。硬规范违反与维护判断分开；气味heuristic repo规则优先，跳过lint：名称不明改名；重复共享；FeatureEnvy移动到拥有者；DataClumps/PrimitiveObsession领域类型；RepeatedSwitch统一映射；ShotgunSurgery聚合；DivergentChange拆模块；SpeculativeGenerality删除无需求抽象；MessageChains隐藏导航；MiddleMan删除纯转发；RefusedBequest用组合。具体file/symbol引用，不凑发现。

Spec写cli-desktop-refresh-spec-review.md。新显式token/upload/share入口是否读写global？失败是否保留类型/脱敏而非假装空队列/未登录？API profile/storage/reading/device/share projection字段是否保留？legacy wrapper兼容？app真正调用新ResultAPI、失败保留snapshot/last_sync/cache，is_loading回收，成功app snapshot/tray更新且不写globalcache？允许暂留global credential/upload桥，不视作完整state迁移。公共tests是否真实提取后Red再Green，fixture均loopback有界/join，无意外外网？GPUI编译/人工不足记限制，不假装验证。报告只覆盖审查时源码，TDD尚在写可注明，勿将WIP缺文档当最终未交付。

父已要求并初查修订：pure device mapper剥离真实WS/LAN副作用、strict统计/device错误保留、endpoint入口必须share projection、逐read绝对deadline、删除private app helper+tests、首次无snapshot不显示空队列。最后writer仍在targeted/docs收尾，审查最终源码，报告若WIP限于该时点。Red是首writer报告的提取后queue503行为；目前只有其文档/口述，没有父可复核原始日志，不得把最终Green当Red证据。两轴仅审本片；不扩到旧6片。
