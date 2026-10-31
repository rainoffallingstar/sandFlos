# 前置 HTTP 取消续实施（2026-10-31）

用户再次要求check progress/TDD/GitHub并继续未完成项；父选择交班中可本机合成验证的前置HTTP取消切片。既有登录交付/Read/后写入取消已完成，不重做。

Own: 新 `core/src/cancellable_prepare.rs`（或扩cancellable_push.rs），services.rs 仅 HttpPushBackend::auth/default_bucket/sts/signed_url及必要imports，api.rs/push.rs 必要pub(crate)纯协议helper共享与lib.rs私有module声明；新增公共S1/S3测试 `crates/cli/tests/pre_write_cancel.rs` 和 `docs/plans/tdd-prewrite-cancel.md`。不得修改PushService、Read/upload、后写入成功/unknown逻辑、auth.rs、桌面或manifest。一个writer，可仅格式化自己完成的代码。

目标：用户timeout300时 users/me、storage、bucket、STS、signed-url verify Range GET 悬停（headers或body）均可SIGINT及时exit130；取消后不开始后续request/第二文件，无盲retry。正常请求、状态优先、401/403/business/protocol分类、脱敏、显式loopback endpoints及旧blocking兼容接口保持。signedURL最多3次现有只读retry/退避仍可取消，不缩短用户timeout、不detached blockingworkers。沿owned async runtime/future模式；新的read-only准备请求无需为了成功结果延长取消，已知提交语义不在此片。

真实Red→Green逐片：先新增一个公共CLI进程慢auth用例，证明收到请求后SIGINT超时（watchdog bounded）而非编译/setup失败，再最小实现，随后逐项补storage/bucket/STS/签名URL phases。复用合成回环夹具、8秒child watchdog、socket≤5秒、signal后≤3秒exit，所有fixture最终结束/回收；不访问真实账号/凭据/书库、不commit/push/tag/release。无需未来接口骨架；不要批量写完测试再补实现。保留Red实际输出日志与命令写入证据。

尽量共享原api.rs已存在的纯请求构造/信封解析、push.rs签名URL构造，避免协议复制。错误不带URL/token/raw响应。保留classify现有映射。只运行针对性测试与回归；父writer结束后运行全量脚本。完成后报告新公共行为、Red/Green命令、通过数和剩余边界。阅读cli-handoff/cli-state-migration/cli-review-and-tdd及AGENTS原始约束。

父审查补充：legacy signed URL verification在200/206响应头时即接受，不消费Range响应body；保持这一契约，不为了测试增加body等待。API信封端点需覆盖body stall，Range GET仅需header stall取消。签名构造沿现有helper。

首轮fixture审查：`join_with_deadline`超时后丢弃JoinHandle会detach；本fixture应stop后真正join，socket/head有界，不能静默detach。单线程accept在hold期间无法观察排队的后续请求；“无后续请求”的断言需要在child退出后释放held response并让accept循环有机会处理已排队连接，再断言，或使用等价有界并发fixture。不要只在accept被hold挡住时断言零请求。可增加SIGINT后立即放行前置响应的用例，验证取消不会由响应完成触发下一request。

中间实现审查（需完成后复核）：新get_json不能直接将cancellable_push::parse_json_response的raw Value作为data；users/storage/buckets/STS均为API信封，必须沿parse_envelope_with_status解析业务码/data且保留status-first。`push.rs`允许必要pub(crate) exposure，请直接共享signed_download_url_with_endpoints、STS阈值与remaining helper，勿复制整段签名算法。credential应在发users/me前按legacy规范normalize/拒空，不能只在请求后trim。

证据分类：`/tmp/prewrite-red-storage.log`当前是“never reached held response，只有GET users/me”，这是中间实现的正常路径回归（raw JSON未解信封），不是storage慢请求取消的行为Red。可以记录该回归如何修复；不得声称已证明storage阻塞取消。若storage与auth已由同个future一起修复，新增storage取消测试首次Green就如实记共享机制回归，不必倒退/故意关闭代码制造Red。
