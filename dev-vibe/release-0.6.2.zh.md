# Hagency Client 0.6.2 发布记录

本轮用户要求把已验证的修复推至远程 main，并发布 0.6.2。沿用本会话既定的未签名、手动安装方式；保留真实 Codex sandbox / two-Agent 资格记录仍为 placeholder、签名和自动更新缺失的限制，不把条件 skip 当作通过。

- 修复提交 761ffdc4f 已快进推送至 origin/main；版本同步 workspace、App、21 个本地锁项、macOS bundle 及未改变 placeholder 状态的版本 pin。第三方依赖零变更。
- 修复图标资源打包目录错配，以及 owner API 把所有失败改成“授权过期”的错误分类。源码修复已有 App 885 passed / 0 failed / 2 pre-existing ignored、授权 HTTP/过期/撤销回归、严格 Clippy 和资源缺失/改写拒绝测试。
- 发布预检整理新增 Engine Rust 代码的格式。严格 real-host 资格检查仍保留拒绝，不改门槛；使用 app-release.yml publish=false 构建并逐个核验产物，准备七个安装包及原始 SHA256 清单后再公开。
- 当前阶段：准备构建，尚未公开。证据位于本工作树忽略目录 .run/release-0.6.2/。

## 构建期间保留的原始异常

- 发布 run41440210562 的 macOS Engine 前两次未获得 hosted runner，GitHub 自动取消；注释明确为 Apple Silicon runner 容量不足。Linux / Windows Engine 原始检查成功，第三次只重试未执行的 macOS Engine 及其下游任务，不把原取消当作通过。
- 常规 App Build41440208999 首次 Windows 默认库872pass/1fail/2ignore：既有 `large_backlog_yields_to_small_room_edit_and_redaction_then_finishes` 在五秒无已提交进度时失败。该路径未被本次改动；完整 Windows 任务原设置重跑通过，未改时间界限或断言，原因未确证，保留原失败日志。Apple Silicon App Build 同样因容量未获得 runner，重试后成功。普通 CI41440209036、App CI41440209319 与 App Build 最新结果成功。
- 独立本机 Engine 补充验证：前两个原始 CI 命令（all-feature no-run、execution fixtures）通过；第三个全包命令只在 `native_octos_owned_refusals_and_bad_peers_close_the_session` 的进程关闭证明断言失败（SignalError，leader已退出、live_count0、whole_tree_stopped=false）。同一工作区特性、同一测试二进制 f8cfa1444e928369 的完整10项 Octos fixture 重跑通过。原整套运行仍为失败、原因未知；未改进程停止或证明规则，也不把所选重跑替代原整套结果。远程发布 CI 是独立结果。

## 实际打包发现并修复的 Python 兼容性遗漏

初始源码 a9e0f7419 的 release run41440210562 已完成三平台 Engine（macOS 获得 runner 后完整测试和 release build 均成功）。Linux ARM64 客户端测试通过后，beforeEachPackageCommand 的 Python 在 `import tomllib` 处报 ModuleNotFoundError；该解释器与先前外层检查所用环境不同。本轮资源修复引入的这个依赖遗漏已在真实打包中被发现，未创建公开0.6.2 Release；剩余旧源任务已取消，不计通过。

新增“不提供任何 TOML 库仍能导入生产 hook”的回归在旧代码失败，修复后6/6通过。现在使用原有 cargo metadata JSON 中匹配当前 manifest 的 library target 名称，既保留显式 lib.name，也不再要求 Python3.11或第三方解析器。匹配其他 workspace 包、资源缺失和字节改写的拒绝仍受测试约束。本机系统Python3.9实际导入 hook并解析真实 Cargo metadata 得到hagency_app成功。新源码将重新推送 main、冻结并重跑发布构建；旧/新源产物和结果不得混用。
