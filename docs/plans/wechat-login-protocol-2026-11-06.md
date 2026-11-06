# 微信扫码登录协议核验（2026-11-06）

本记录服务于 `booxctl auth login --qr` 默认微信登录切片。测试只使用合成回环数据；本文的线上探测只创建一次性二维码会话，没有扫码、登录、读取账号、推送或保存凭据。二维码会话 UUID、`state`、微信授权码和任何令牌均未写入本文。

## 结论

实现必须按现行微信网页登录协议处理以下差异，不能用旧的 BOOX 助手二维码流程代替：

- 页面入口是 `https://open.weixin.qq.com/connect/qrconnect`，二维码图像是 `/connect/qrcode/<uuid>`。
- 线上二维码图像实际为 **JPEG**（`image/jpeg`，JFIF 魔法 `ff d8`），不是 PNG。实现应支持现行 JPEG，并在格式、尺寸或完整解码失败时返回协议错误；不要仅凭扩展名或旧注释假定 PNG。
- 会话 UUID 可由同一 `qrconnect` 请求追加 `f=xml` 获得（XML 的 `<uuid><![CDATA[...]]></uuid>`），也可从页面中的 `/connect/qrcode/<uuid>` 提取。两种请求得到不同的一次性 UUID。
- 当前 XML 指示 `usenewdomain=1`，页面轮询主机选择为 `https://lp.open.weixin.qq.com/connect/l/qrconnect?uuid=<uuid>`；旧 `https://long.open.weixin.qq.com` 是回退主机。不能固定只轮询后者。
- 轮询响应是 `text/javascript` 形式的赋值脚本，例如 `window.wx_errcode=408;window.wx_code='';`。只能解析赋值，不能执行脚本。
- 观察到的空闲轮询约 15 秒后返回 `408`；连续请求可带 `last=408`。`405` 表示授权成功且 `wx_code` 可用；`404` 为已扫描待确认；`403` 为拒绝/取消；`402`、`500` 视为二维码过期。其他或缺失状态必须报协议错误。
- `405` 后应使用页面固定的 `https://send2boox.com/api/1/passport/wechat/pc/login?code=<wx_code>&state=bind_login_<nonce>` 回调；不得跟随任意重定向或执行返回内容。使用合成无效码时线上返回 HTTP `413` 和 `{"result_code":1,"message":"微信登录失败","data":null}`；缺少 state 返回 HTTP `500` 与 `state is not found!`。实现应对非成功状态返回认证或协议错误，不能把 HTTP 状态当成功。
- WebSocket 为 `wss://send2boox.com/ws/socketio/?state=<nonce>`，`state` 去掉 `bind_login_` 前缀。连接建立后线上发送 `{"action":"message","message":"<nonce> joined!","room":"<nonce>"}`；成功凭据消息的契约来自既有代码：`{"action":"pc_login_finish","data":{"token":"..."}}`。必须先连接并完成房间加入，再展示二维码和处理回调，避免手机快速确认时丢失令牌。
- WebSocket 在 40 秒空闲观察内没有额外保活文本；实现仍必须回复 ping 并限制单条消息大小。未知 action、坏 JSON、连接关闭、超时或缺失 token 都不能保存凭据。

## 可直接复现的探测

使用临时 Python 环境中的 `requests`、`Pillow` 和 `websocket-client`。以下命令只打印脱敏后的协议特征；不要复用或记录其中的会话标识。

```bash
python3 /tmp/wechat_probe.py
python3 /tmp/wechat_probe2.py
python3 /tmp/wechat_probe3.py
python3 /tmp/wechat_probe4.py
python3 /tmp/wechat_poll_check.py
python3 /tmp/wechat_ws_idle.py
```

这些临时脚本不是产品依赖，也不能作为仓库测试。可长期复核的关键观察如下：

| 项目 | 观察 |
| --- | --- |
| `qrconnect` HTML | HTTP 200，约 43 KiB，`text/html; charset=utf-8` |
| `qrconnect?...&f=xml` | HTTP 200，315 字节，`text/xml; charset=utf-8`，UUID 在 CDATA 中 |
| 二维码图像 | HTTP 200，约 46–47 KiB，`image/jpeg`，JPEG `470x470`，灰度 26 个值 |
| 图像几何 | 47×47 模块、每模块 10 像素，四周 3 模块静区；暗模块边界 `(3,3)` 到 `(43,43)` |
| 新轮询主机 | `lp.open.weixin.qq.com`，空闲约 15 秒后 `wx_errcode=408`，响应 40 字节 |
| 旧轮询主机 | `long.open.weixin.qq.com`，当前也可返回相同 `408` 响应 |
| 二维码图像其他主机 | `lp`/`long` 的 `/connect/qrcode/<uuid>` 返回 HTTP 404 |
| 回调无效码 | HTTP 413，JSON `result_code:1`，`data:null` |
| 回调缺 state | HTTP 500，JSON `state is not found!` |
| WebSocket 握手 | 立即成功；首个文本消息为房间加入确认 |

线上观察是一次性会话，不能证明所有微信边缘节点的图像格式、主机选择或状态码永久不变。实现应将格式、主机选择、响应上限和错误分类写成有界、可测试的协议边界；回环测试覆盖成功、待扫描、拒绝、过期、错误 HTTP、坏 JSON、缺 token、取消和 WebSocket 关闭。真实扫码与令牌保存仍属于人工验收，不能由这些探测代替。

## 对当前实现的审查点

- 旧注释和 `/tmp/booxctl-wechat-implementation.md` 中的“PNG”不准确。仅使用 `png` 解码会让当前线上流程在第一步失败；若同时接受 PNG 与 JPEG，应按实际内容类型和魔法字节选择解码器，不能信任 Content-Type 单独判断。
- 页面 `var U=window.usenewdomain?...` 是动态选择。实现应从 XML 的 `usenewdomain` 或同一固定页面逻辑选择轮询 origin；生产 origin 必须固定为微信官方域名，不接受任意跳转或 URL 注入。
- 当前页面 XML 的 315 字节体和图像约 47 KiB，均远低于父任务的 512 KiB HTML、1 MiB 图像上限；仍要按读取字节数设硬上限。
- 二维码图像存在 JPEG 压缩噪声，按像素游程求 GCD 会得到 1，不能直接当 PNG 的无损模块网格。应阈值化后按图像几何/静区/模块中心采样验证 47×47 网格；若用 470/10 直接推断，需拒绝非精确网格、静区不符合或尺寸异常图像。
- 回调失败可能是 HTTP 413/500，而 WebSocket 也可能随后关闭。实现应优先返回确定的回调/协议错误，不能在没有成功令牌时假装登录成功。
- 回调成功后要检查取消标志，再等待并验证 WebSocket 令牌，然后用现有 `AuthService.import_token_cancellable` 校验和持久化；令牌、微信码、state 和原始响应正文不得进入 stdout、日志或错误文本。
- CLI 的 `--timeout` 是单次请求超时；整体二维码会话仍应有独立、有限的总期限（既有契约为 5 分钟）。轮询应立即返回的 `408` 需要退避，不能形成 CPU 忙循环。

## 未验证

- 真实微信扫码、手机确认、BOOX 回调成功以及 `pc_login_finish` 携带真实令牌的完整链路；本轮没有登录任何账号。
- 微信不同地区/边缘节点的图像格式与轮询主机切换；只验证了当前会话。
- WebSocket 长时间空闲、网络重连、服务端 ping 的实际间隔；本次 40 秒观察没有额外保活消息。
- 成功回调后 WebSocket 消息的到达时序和重复消息行为；实现仍需防止重复持久化。

以上未验证项必须保持为人工验收或后续真实账号测试的阻塞项，不能因代码测试通过而标为通过。
