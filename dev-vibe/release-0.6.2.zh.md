# Hagency Client 0.6.2 发布记录

本轮用户要求把已验证的修复推至远程 main，并发布 0.6.2。沿用本会话既定的未签名、手动安装方式；保留真实 Codex sandbox / two-Agent 资格记录仍为 placeholder、签名和自动更新缺失的限制，不把条件 skip 当作通过。

- 修复提交 761ffdc4f 已快进推送至 origin/main；版本同步 workspace、App、21 个本地锁项、macOS bundle 及未改变 placeholder 状态的版本 pin。第三方依赖零变更。
- 修复图标资源打包目录错配，以及 owner API 把所有失败改成“授权过期”的错误分类。源码修复已有 App 885 passed / 0 failed / 2 pre-existing ignored、授权 HTTP/过期/撤销回归、严格 Clippy 和资源缺失/改写拒绝测试。
- 发布预检整理新增 Engine Rust 代码的格式。严格 real-host 资格检查仍保留拒绝，不改门槛；使用 app-release.yml publish=false 构建并逐个核验产物，准备七个安装包及原始 SHA256 清单后再公开。
- 当前阶段：准备构建，尚未公开。证据位于本工作树忽略目录 .run/release-0.6.2/。
