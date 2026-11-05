# Hagency Client 0.6.5 发布记录

## 准备状态（2026-11-05）

操作者要求合并左侧审批修复、拉取远端最新 main，并推送下一版本。PR #51 已 squash 合并为 `4067b6faf46360f0f12dc5679aaecc38ec12c66c`，本地 main 已从 origin 快进至该提交。合并时部分远端检查仍在运行，没有把 pending 或 skip 记作通过；快照保存在忽略目录 `.run/release-0.6.5/`。

本次补丁版本为 0.6.5，包括 PR #49 的 Agent 附件接收与私聊运行状态修复，以及 PR #51 的左侧审批弹窗输入顺序修复。workspace/App/macOS bundle 版本一致；Cargo.lock 仅更新 21 个本地包版本，第三方锁项、版本、校验和及依赖关系不变。two-Agent 版本 pin 同步为 0.6.5，原 `placeholder: true` 明确保留，没有伪造资格证据。

## 验证依据

- 原修复完整 App library：886 passed、0 failed、2 项既有 ignored；审批发现组件 12 passed；App no-deps 严格 Clippy 和锁定依赖的原生构建通过。
- 公网 `hagency.meldry.com`：附件 receive/read/send/sent/download 字节一致；左侧待办出现、精确跳转、批准、拒绝和完整 300 秒自然过期通过。批准分支实际调用模型 7280 token，拒绝和过期无模型调用；服务已停止、编程执行关闭、工具 Deny、held0。详见两个随源码保存的实测报告。
- 本轮版本准备：资源收集回归 6 passed，发布附件清单 3 passed，资格门槛测试 4 passed，图标检查及锁定/离线的完整 Cargo metadata 解析通过。
- 系统 Python3.9 不具备 tomllib，本轮发布校验使用已安装的 Python3.12。cargo update --workspace --offline 会尝试解析本机没有的未使用旧 patch revision，原失败保留；仅调整本地 lock 包版本后，以完整 locked/offline metadata 确认实际依赖图，不更新第三方依赖或改写缺失 patch。

## 尚未通过的严格发布资格

`tools/native/check-release-qualifications.py` 实际拒绝：Codex sandbox 与 two-Agent 两条 real-host 记录仍为 placeholder。仓库没有配置签名/自动更新所需的有效 repository secrets/variables 清单。当前正式 0.6.4 采用未签名、手动安装方式，不包含自动更新或上述严格资格。

候选产物使用现有 `app-release.yml` 的 `publish=false` 构建；qualification 与自动 publish 的条件 skip 仅记为 skip。所有原 Engine 平台检查、App 测试、四平台打包以及六包哈希/内容核验均需保留实际结果；未通过时不得把候选当作完整验收。不得覆盖或移动已经发布的版本标签。

后续实际构建、产物核验、发布状态以追加记录为准。原始凭据、profile、私有日志和下载证据均只保留在忽略目录。

## 首次候选 Windows 发布环境失败及修复

首个候选源码 `9fece4317efd265bd0c995276894cda29212bce7`、run41965049377 的 Windows2022 Installer 在 App 测试失败：858 passed、23 failed、2 ignored。失败集中在凭据写入/恢复的 `credential_commit_durability_unknown`，新增审批鼠标回归在该真实 Windows 任务中通过。原始完整 Windows 日志及失败前全部 job 状态已保存，不把失败改记为通过；剩余旧源任务主动取消，不混用其产物。

发现发布流程仍把 TEMP/TMP 与大型构建缓存一起放在 D 盘，而普通 Windows App Build CI 已有明确规则：部分 hosted2022 D 盘带 FILE_PORTABLE_DEVICE，凭据目录持久化适配器必须拒绝；测试临时数据使用系统卷 `C:/hagency-ci-tmp`。本次修复让 Installer 使用相同已存在的测试卷规则，CARGO_HOME、target 和打包工具仍在 D 盘，并在 App 测试前运行已有 `windows_credential_sync`，实际证明目录 flush 及安装凭据写入/重开。没有放宽存储能力判断、文件权限或测试断言，也没有省略凭据测试。

actionlint、原工作流 Python 表达式的 Windows/非 Windows 路径检查及 diff whitespace 检查通过；新候选必须重新完整构建并取得真实 Windows probe 和 App 测试结果。原机器未记录具体原生卷 flags，因此不把该已知 D 卷类别推断描述为本次机器的实测 flags。产物和结果将使用独立 `.run/release-0.6.5-r2/`。
