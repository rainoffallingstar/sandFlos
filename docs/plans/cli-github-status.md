# GitHub 构建状态核对

2026-10-31：用户最新明确允许父提交代码，已将此前七片已验收改动提交为 `d3810d0fb5d9646bd18b49da8535a0c6c5cd52bc` 并发布到远端cli。Git HTTPS连接github.com:443失败；父经GitHub Git Data API发布相同blob/tree/commit SHA并非强制更新ref，远端API已核实相同SHA，没有改写历史。

对应run **41190025958，7/7 success**：Linux/macOS/Windows CLI三项、下载产物原生运行三项、macOS desktop-compatibility一项。覆盖的是d3810d0；后续桌面session工作区尚需新SHA CI。本机缺aes缓存的桌面类型检查缺口，已由本次macOS cargo check补齐，不代表人工桌面回归。

旧c460ab6/run41024736353仅为历史，不再作当前最新验收。本轮不创建tag/release或使用真实业务账号。

2026-10-31 方向变更：`cli` 分支后续只做纯 CLI，不再接入桌面端。未提交的桌面 session 工作区已撤回，源码/测试/TDD 文档归档在工作区外的 `/tmp/sandflos-desktop-session-wip`（`tracked.patch` + 未跟踪文件副本），未提交、未推送。因此上面 run 41190025958 的 desktop-compatibility 一项只代表旧 SHA 的历史结果，不再是本分支的目标。工作区现无桌面 session 改动，也没有对应新 SHA/run。
