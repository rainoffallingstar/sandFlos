# GitHub 构建状态核对

2026-10-31：用户最新明确允许父提交代码，已将此前七片已验收改动提交为 `d3810d0fb5d9646bd18b49da8535a0c6c5cd52bc` 并发布到远端cli。Git HTTPS连接github.com:443失败；父经GitHub Git Data API发布相同blob/tree/commit SHA并非强制更新ref，远端API已核实相同SHA，没有改写历史。

对应run **41190025958，7/7 success**：Linux/macOS/Windows CLI三项、下载产物原生运行三项、macOS desktop-compatibility一项。覆盖的是d3810d0。该 run 的 desktop-compatibility 一项已随纯 CLI 方向移除，仅作历史。

旧c460ab6/run41024736353仅为历史，不再作当前最新验收。本轮不创建tag/release或使用真实业务账号。

2026-11-01 纯 CLI 方向已落地并验证：提交 `0113905`（CI 去桌面、README/docs 改写、方向文档）与 `66b9bf2`（去掉慢读进度用例的超时脆弱点）推送到 `origin/cli`。对应 run **41257052317，6/6 success**：ubuntu/macOS/windows 三项 CLI、三项下载产物复验；已无 desktop-compatibility 项。首次 run `41255922160` 在 macOS 因既有 `transfer_streaming` 慢读用例的 10 秒超时余量不足而失败，已在 `66b9bf2` 修正并复跑通过。

2026-10-31 方向变更：`cli` 分支后续只做纯 CLI，不再接入桌面端。未提交的桌面 session 工作区已撤回，源码/测试/TDD 文档归档在工作区外的 `/tmp/sandflos-desktop-session-wip`（`tracked.patch` + 未跟踪文件副本），未提交、未推送。因此上面 run 41190025958 的 desktop-compatibility 一项只代表旧 SHA 的历史结果，不再是本分支的目标。工作区现无桌面 session 改动，也没有对应新 SHA/run。
