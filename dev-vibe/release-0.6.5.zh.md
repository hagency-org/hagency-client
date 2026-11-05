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

## macOS 14 runner 退役，更新第三次候选

第二候选 `4f10c555389578d8afe4d70df07d4a9e2a24a5ee` / run41967223872 已实际通过 Windows 目录持久化探针与完整 App 测试（probe04:25:16Z、App04:33:55Z），两个 Linux Installer 的 App 测试也通过；其 macos-14 Installer 长时间未分配 runner。没有把这些部分结果记为完整发布验收。

核对 [GitHub 官方退役公告](https://github.com/actions/runner-images/issues/13518) 和 [当前 runner 镜像表](https://github.com/actions/runner-images) 后确认：全部 macOS14镜像已于2026-11-02从 GitHub Actions退役；当前 macos-15 是支持的 arm64 runner。发布矩阵将 macOS arm64 Installer 从 macos-14 更新到 macos-15，其他平台、命令、断言、期限和发布门槛不变。旧候选取消并保留实际状态，新冻结源码必须重新跑完整构建，产物使用 `.run/release-0.6.5-r3/`，不拼接旧候选产物。

PR51 的检查此时全部结束：12项实际检查通过、6项条件跳过，完整快照保留在第二候选证据目录。


## 最终候选构建及安装包核验

最终构建源码为 `207681a435ea88ba5a8f23b213afbf5a4ed26dde`，本地和 origin/main 均已同步。独立第三轮 [run41970002844](https://github.com/hagency-org/hagency-client/actions/runs/41970002844) 实际完成且 success：Ubuntu24、macOS15、Windows2025 三个完整 Engine 任务，以及 Linux amd64/arm64、macOS arm64、Windows amd64 四个 Installer 任务均 success。Windows Installer 的实际凭据目录持久化探针通过。正式 real-host qualification 与自动 Publish 两项因 `publish=false` 条件跳过，明确不是资格通过。

四个平台均通过 10 项 App contract 测试。macOS 和两个 Linux 平台的 App library 各 886 passed、0 failed、2 existing ignored；Windows 对应平台选择器为 881 passed、0 failed、2 existing ignored。新增 Home 审批鼠标路由回归在四个平台的原始日志中都通过。

四个 Actions 安装包归档均先核对 API 提供的完整归档 SHA-256，随后解包检查六个实际安装包。单连接下载严重缓慢时改用标准 HTTPS Range 分段，仍以合并后的完整归档哈希作判据。每个安装包的版本、真实可执行架构、内嵌完整源码 permalink 与 139 个 `hagency_app/resources` 资源路径/字节均一致。DMG 的 Info.plist 及实际 Mach-O 最低版本均为 macOS12.0；此检查不冒充真实 macOS12 硬件运行资格。检查使用只读解包，不运行任何安装器、不挂载 DMG 或代替用户接受安装许可。

| 安装包 | SHA-256 |
| --- | --- |
| `hagency-linux-amd64.AppImage` | `ad8f464e7cf6e9fcc09b10796326b1d737cc91e804e85f79a73c19088e345b7b` |
| `hagency-linux-amd64.deb` | `7d5447b24290d7e62d8bc206f3b6fbdad2717ae533a36f5a86374426daf21687` |
| `hagency-linux-arm64.AppImage` | `7c2acb0f0e69106266e4cf803941c144641ee4aeed5f876489e404d663f30805` |
| `hagency-linux-arm64.deb` | `e27c466ad4b43243cc16d0461f318914a0e31ffdcf12bce09456be151a366e7a` |
| `hagency-macos-arm64.dmg` | `394395a1d68a6cc8dc7af086e3855fc781440fe5eae5512cf252e774cd251a21` |
| `hagency-windows-amd64.exe` | `247f6f0412a5044026853dad0153b79f0066266192010b6af11733fc6263955f` |

六包加四份平台校验清单及一份总清单，完整附件集共 11 项。冻结源码的严格资格预检再次实际 exit1，仍拒绝两条 placeholder real-host 记录；未伪造记录、未改变发布门槛，也未把 skip 当作通过。当前仅准备未签名、手动安装草稿，公开发布和远端附件字节复核状态以后续记录为准。


## 完整草稿已核验，等待本次发布模式决定

GitHub [0.6.5 草稿](https://github.com/hagency-org/hagency-client/releases/tag/untagged-5db4a23a6a2b12ee6ab1)，release ID434851444，target 完整绑定冻结源码 `207681a435ea88ba5a8f23b213afbf5a4ed26dde`。全部 11 项附件均完成服务器 state/size/SHA-256 核对，并从 GitHub 重新认证下载，逐项与原始本地产物字节/摘要一致。草稿内容明确三项产品修复、公网实测范围、未签名手动安装、无自动更新及两项未完成 real-host 资格。原始运行结果及核验收据均留在忽略目录 `.run/release-0.6.5-r3/`，不上传原始 profile、凭据或私有日志。

此时尚未公开发布；正式 Latest 仍为0.6.4。公开升级需单独决定本次是否沿用未签名、手动安装方式，不能把既往某一版的例外授权永久化，也不能宣称严格资格通过。文档提交可以晚于产物源码，发布标签必须保持指向实际构建的冻结源码，不得移到文档提交。
