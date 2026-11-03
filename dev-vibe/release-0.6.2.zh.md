# Hagency Client 0.6.2 发布记录

## 最终公开发行（2026-11-03）

- 已公开：[Hagency Client v0.6.2](https://github.com/hagency-org/hagency-client/releases/tag/v0.6.2)，GitHub Release id432132711；发布时间2026-11-03T01:01:58Z（上海2026-11-03T09:01:58+08:00），Latest。
- 实际构建/标签源：`6874517354c3f6c9ba763b1cfde2c2ee967c220b`；[成功run41543120369](https://github.com/hagency-org/hagency-client/actions/runs/41543120369)。原3平台Engine与4组Installer全部实际成功，原始运行attempt1；qualification和自动publish两项条件skip不计通过。
- 新范围为Linux amd64/arm64 DEB+AppImage、macOS arm64 DMG、Windows amd64 EXE，共6包及4平台清单/总清单，11附件。安装包共863132794字节。每包正确运行时目录139个桌面资源与该源码逐字节一致；版本/内部架构、原始SHA清单均通过只读解包校验，未执行安装器。
- 草稿11附件的GitHub服务端digest/大小/state及全部认证下载字节与本地原件一致后才公开。独立回读公开Release、Latest、lightweight tag目标、11digest、匿名总清单和6个Latest安装包入口全部通过。tag触发重复严格流水线41556353158已completed/cancelled；不计通过，实际来源仍是成功run41543120369。
- PR44于2026-11-02T23:07:59Z由@chrislearn squash合并为687；当时部分旧PR ARM检查尚未结束，不能称合并时全绿。667构建41530365979和PR预览41532739429手动取消，未重启或混用产物；随后此main源重新完整发行验收。普通CI41542982243、App CI41542982335、App Build41542982218和Spelling41542982284实际成功；后两者原ARM runner未获得执行的取消留档，单job重排后实际成功。
- 沿用用户确认的未签名、手动安装方式；不含Developer ID/notarization、Windows签名、自动更新或两项真实环境qualification。历史实际失败及未知原因均保留下文与Release说明，不将重跑/取消/skip解释为原失败通过。


本轮用户要求把已验证的修复推至远程 main，并发布 0.6.2。沿用本会话既定的未签名、手动安装方式；保留真实 Codex sandbox / two-Agent 资格记录仍为 placeholder、签名和自动更新缺失的限制，不把条件 skip 当作通过。

- 修复提交 761ffdc4f 已快进推送至 origin/main；版本同步 workspace、App、21 个本地锁项、macOS bundle 及未改变 placeholder 状态的版本 pin。第三方依赖零变更。
- 修复图标资源打包目录错配，以及 owner API 把所有失败改成“授权过期”的错误分类。源码修复已有 App 885 passed / 0 failed / 2 pre-existing ignored、授权 HTTP/过期/撤销回归、严格 Clippy 和资源缺失/改写拒绝测试。
- 发布预检整理新增 Engine Rust 代码的格式。严格 real-host 资格检查仍保留拒绝，不改门槛；使用 app-release.yml publish=false 构建并逐个核验产物，准备七个安装包及原始 SHA256 清单后再公开。
- 初始准备阶段的原始记录为尚未公开，证据保留于本工作树忽略目录 .run/release-0.6.2/；最终结果见上文，发行原件与完整收据位于 .run/release-0.6.2-r6/。

## 构建期间保留的原始异常

- 发布 run41440210562 的 macOS Engine 前两次未获得 hosted runner，GitHub 自动取消；注释明确为 Apple Silicon runner 容量不足。Linux / Windows Engine 原始检查成功，第三次只重试未执行的 macOS Engine 及其下游任务，不把原取消当作通过。
- 常规 App Build41440208999 首次 Windows 默认库872pass/1fail/2ignore：既有 `large_backlog_yields_to_small_room_edit_and_redaction_then_finishes` 在五秒无已提交进度时失败。该路径未被本次改动；完整 Windows 任务原设置重跑通过，未改时间界限或断言，原因未确证，保留原失败日志。Apple Silicon App Build 同样因容量未获得 runner，重试后成功。普通 CI41440209036、App CI41440209319 与 App Build 最新结果成功。
- 独立本机 Engine 补充验证：前两个原始 CI 命令（all-feature no-run、execution fixtures）通过；第三个全包命令只在 `native_octos_owned_refusals_and_bad_peers_close_the_session` 的进程关闭证明断言失败（SignalError，leader已退出、live_count0、whole_tree_stopped=false）。同一工作区特性、同一测试二进制 f8cfa1444e928369 的完整10项 Octos fixture 重跑通过。原整套运行仍为失败、原因未知；未改进程停止或证明规则，也不把所选重跑替代原整套结果。远程发布 CI 是独立结果。

## 实际打包发现并修复的 Python 兼容性遗漏

初始源码 a9e0f7419 的 release run41440210562 已完成三平台 Engine（macOS 获得 runner 后完整测试和 release build 均成功）。Linux ARM64 客户端测试通过后，beforeEachPackageCommand 的 Python 在 `import tomllib` 处报 ModuleNotFoundError；该解释器与先前外层检查所用环境不同。本轮资源修复引入的这个依赖遗漏已在真实打包中被发现，未创建公开0.6.2 Release；剩余旧源任务已取消，不计通过。

新增“不提供任何 TOML 库仍能导入生产 hook”的回归在旧代码失败，修复后6/6通过。现在使用原有 cargo metadata JSON 中匹配当前 manifest 的 library target 名称，既保留显式 lib.name，也不再要求 Python3.11或第三方解析器。匹配其他 workspace 包、资源缺失和字节改写的拒绝仍受测试约束。本机系统Python3.9实际导入 hook并解析真实 Cargo metadata 得到hagency_app成功。新源码将重新推送 main、冻结并重跑发布构建；旧/新源产物和结果不得混用。

## 反复 runner 容量失败后的 CI 调整

修复 Python 后的新候选73f735701 / run41463685131，macOS Engine 再次因“not acquired by Runner”未执行即取消；原注释存于 .run/release-0.6.2-r2/mac-engine-cancellation.json。加上前一候选的两次同类失败，ARM Engine runner 获取已累计三次失败，阻断整个依赖链。终止未完成的旧候选，不计通过。

Engine 三平台矩阵继续覆盖 Linux、macOS、Windows，将 macOS15 的 Engine runner 固定到官方 macos-15-intel，以绕开反复不可分配的 ARM 池；全套命令、选择器、断言和单项超时不变。Intel macOS 的整体冷编译/测试 job 预算同 Windows 设为90分钟，Linux仍60分钟。五组安装包任务未改变：Apple Silicon 继续在原生 ARM runner 完整 App 测试和打包，Intel 继续原生 Intel 测试和打包。最终候选与证据使用独立 .run/release-0.6.2-r3/。

## Intel Engine runner 实验失败，恢复原有矩阵

候选357b785a2 / run41467014503 的 Intel macOS Engine 完整套件实际失败，不计通过：hagency lib301pass/2fail/9ignore（cookie_rotation_preserves_inflight_management_reply、slow_management_does_not_block_renewal_and_late_logout_discards_reply，均未在原一秒内观察到管理请求启动）；hagency-matrix lib279pass/1fail（native_provisioning_inline_home_custody，原十五秒工作收尾 watchdog 触发）；claude_session2pass/1fail（backpressure_and_capacity 得到Timeout而非Capacity）。未确证具体原因，不能仅凭资源架构推断它们为代码正确或平台抖动。

停止这次替代 runner 实验，原样恢复已使用的 Engine macOS ARM矩阵及原任务预算；不调整任何测试命令、断言、单项期限或跳过选择器。保留 Intel 实验为失败的补充结果，不声称 Intel Engine 集成验收通过。Intel安装包仍需原生Intel完整App库测试/构建，最终发布依赖原有三平台Engine与五平台安装包流水线的真实结果。新源与产物用独立 .run/release-0.6.2-r4/；旧源日志、取消和失败均保留。

## 用户调整发布范围：停止 macOS amd64 打包（2026-11-03）

用户明确要求以后不再打包 macOS amd64。当前未发布0.6.2同步采用新范围：Linux amd64/arm64两种格式、macOS arm64 DMG、Windows amd64 EXE，共四平台六安装包；未签名发行共11附件（六安装包、四平台清单、总清单）。移除 release 工作流的 Intel 安装包矩阵，调整总清单数量、资源收集器、Sparkle完整资产/远端校验和发布说明。普通 Intel 编译/测试保留，Engine原三平台命令/断言/预算不变。

旧源81eeb2dd2的run41478120359最终已实际通过三个Engine与四个Installer，Intel Installer仍在打包；未创建tag/Release。完整状态、六个已核验安装包（每包139个源码资源逐字节一致）保留在.run/release-0.6.2-r4；未完成的旧源流水线将取消，不视为全通过，不把其产物混入新源。修改后从新冻结源重新构建四平台，并在全部实际成功及六包完整校验后公开。新源证据使用独立.run/release-0.6.2-r5。

Validation: actionlint for app-release.yml and git diff --check pass; installer collection tests3/3 and macOS signed-feed tests7/7 pass, including the real pinned Sparkle signer format check. Fixtures do not establish Developer ID/notarization or public update qualification.
