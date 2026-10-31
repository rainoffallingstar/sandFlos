# 桌面刷新迁移第一片

2026-10-31，基线3cc6fb7+当前working/untracked，已有6片255Rust+7Python完整检查通过，不重做。读AGENTS共同限制、handoff/state-migration/review-and-tdd。用户指定DeepSeek V4.1 Flash high实施，父集成审查；S2/S3已确认。无commit/push/tag/release/真实账号凭据书库。

下一步计划分三片：本片桌面刷新显式凭据/状态及错误保留；后续登录回调/持久化及app-owned会话；最后上传互斥/进度/书库路径迁移。先完成本片，不声称完整app迁移。允许必要重构，保留旧公共兼容入口/用户缓存。

Own core/dashboard.rs、必要device/api endpoint helper小patch、app/state.rs刷新hunk及必要字段/import、新core公共S2/S3 desktop_refresh测试、docs/plans/tdd-desktop-refresh.md。不改CLI/其他services/PushService/authportal/manifests。需要额外共享编辑先报父。

实现可返回Result或明确逐字段错误的显式dashboard入口，接收token及上传投影/endpoint测试上下文，不读取/写入core全局auth/upload/cache。共享原协议helpers，保留profile、storage、reading、device+share_devices合法互传等桌面数据，不为简化测试丢产品字段。不吞queue错误为空列表、也不把非认证网络错误伪装未登录；缺统计值null保持原义。失败可整体返回类型化错误，app保留旧snapshot并显示错误，失败不更新时间/cache；首次失败也需可见通知。成功由app持有snapshot，不写core dashboardcache。登录/上传仍legacy过渡并文档明确；刷新若仍通过兼容函数取得credential/upload projection，标清这是暂存桥，不能称全局state清零。

TDD必须先公共行为Red：用最小可运行adapter/legacy公开路径证明queue HTTP失败被显示空队列（不可编译缺API不算Red）；不要真实默认地址/全局真实cache。可先把legacy聚合提取到可注入endpoint公开入口但保持旧吞错行为，再测试观察Red，标为提取后行为Red而非原始baseline直接可运行。随后最小Green，不提前写全矩阵。新公开API测试验证queue503/invalidpayload不报成功空列表、身份401与网络分类/secretfree、成功projection/本地日期参数/空队列区分错误、显式token且无全局状态依赖。所有HTTP均literal loopback，包含share_devices不许意外外网。夹具每read绝对deadline，write/child有界，实际join。后续共享机制直接Green诚实记录。

App刷新接入实际新API：结果成功更新app snapshot/tray/last_sync，失败保留现有snapshot并notification，is_loading必须回收。检查GPUI类型兼容，可尝试cargo check -p send2boox-desktop-gpui，但Linux未配置显示backend可能失败应记录环境限制，不能当Red或声称macOS编译成功。避免新依赖。writer只targeted+clippy，父结束后完整check_cli。不要cargo fmt整个app历史文件导致无关改动。

## 父初审修订（完成前必读）

1. with_endpoints回环入口不得因share_devices=None隐式连接production WS。生产发现只能在production wrapper/明确provider中触发；端点注入入口只消费share projection或拒绝缺少输入，保留production功能且不重复身份请求。
2. strict新刷新路径必须保留readingInfo/week/day/devices错误，不再unwrap_or(Null)；HTTP200的data:null仍合法缺失。legacy兼容入口可保留历史soft fallback，明确标记区别。
3. fixture BufRead::read_line内部可能多次底层read，单次调用前设置timeout不能保证绝对deadline。用query_cancel最终的逐read检查（或byte读取）方案；写/头/body/线程有界并真正join。
4. 第一项真实Red后后续机制用例directGreen诚实记录；不要回滚实现制造test-first历史。

5. app新增的private apply_refresh_result helper及其两个私有单测不符合S2/S3。删除这些新增私有测试；直接在实际this.update里match结果，避免5个可变引用的参数团。公共HTTP测试验证Result，app wiring静态审查及可行编译；不要临时把helper pub伪装公共seam。

6. build_dashboard_devices有隐式真实LAN副作用：collect_local_ipv4/arp、detect_local_ipv4(8.8.8.8)、warmup_arp_by_udp_sweep。新with_endpoints必须使用纯device projection（显式share+空本地环境或显式环境输入），production wrapper/legacy才保留LAN探测。允许device.rs小重构抽共享映射，禁止复制映射或测试意外探测真实LAN；生产LAN能力须保留。

7. 首次刷新失败时views/push.rs把snapshotNone映射emptyvec后无条件显示“队列为空”，仍误报。允许此文件小hunk修正空状态：is_loading读取中；snapshotNone显示尚未取得队列数据/查看刷新提示；只有Some合法快照空vec才“队列为空”。已有旧snapshot保留+通知报错可接受。本片无新增私有UI测试，静态审查该分支。

## 父最终验收（2026-10-31）

七项初审修订及production忽略显式share问题已关闭；两轴无确认硬规范或方案缺陷，cache测试隔离维护发现已关闭。完整267Rust+7Python及CLI检查通过，测试夹具最终修订后12项公共回归/clippy/fmt通过。GPUI依赖阻塞、首writer Red无原始日志、完整app-owned会话和当前代码三平台CI均明确保留，不能将整体计划标完成。见handoff最新节及next-steps-and-debt。
