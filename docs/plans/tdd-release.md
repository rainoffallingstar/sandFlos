# CLI 独立发布切片

日期：2026-10-30；沿用已确认的 S4 安装产物边界。本记录只覆盖这轮新增发布流程，不将已有产物测试重新记为 Red/Green。

## Red / Green

1. `python3 -m unittest discover -s scripts -p test_release_cli.py`：先增加标签版本不匹配时失败且不创建目标目录的进程验收。Red 为缺少 `release_cli.py`，最小实现加入 CLI manifest 版本核对后 Green（1 项）。
2. 同一命令加入真实 ZIP 运行与重复归档保留旧文件的验收。Red：正确标签仍报 `archive creation is not implemented`（退出 2）。实现原生包运行校验、平铺 ZIP、归档 SHA-256、拒绝覆盖后 Green（2 项）。POSIX 测试使用可执行 shell 样本，Windows 原生可执行文件另由 CI 完成。

工具链安装：首次受代理拒绝连接影响失败，清除该命令的代理变量后安装 Rust 1.97.1 成功；此前 stable 也为同一 rustc 版本。安装失败不计作 TDD Red。新增 `rust-toolchain.toml`，CLI 与桌面兼容性 CI 显式使用相同版本。

## 发布契约

- 独立 `release-cli.yml`，仅 `cli-v*` tag 与显式 workflow_dispatch 触发，不改桌面发布流程。
- 标签必须与 CLI manifest 完全匹配；无非幂等重试、无覆盖标签或已有 archive。
- 三平台执行统一检查；原生 ZIP 再次按 S4 解压、核验 SHA-256 与运行；单独上传下载文件校验和。
- 现有 `cli.yml` 的三平台检查也调用同一归档脚本并验证生成的版本化 ZIP，上传 ZIP 和外部校验和，便于在不创建标签时验收发布产物。
- tag 触发且三平台全绿才创建 draft release；手动触发只验证和上传产物，不创建发布或标签。
- 本轮不创建真实发布标签。实际 tag 事件与维护者发布 draft 的验收仍需另行执行。
