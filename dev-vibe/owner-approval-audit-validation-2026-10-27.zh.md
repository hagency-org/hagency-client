# 主人审批审计实施与验收

2026 年 10 月 27 日，`codex/agent-capabilities`。本批完成 A6 的审批审计部分；停止任务检查、文件现状摘要、主人处置备注与显式继续仍是后续工作，A6 和整体 A2–A7 目标继续进行。

## 已接通的产品链

- 本设备 SQLite 保存完整原提案、SHA256、Agent/Room/请求者/话题/dispatch、规范工作目录、策略版本、有效期及独立随机审批编号。相同提案的两次请求保留两条记录。
- 事件只追加：等待决定、主人/策略允许或拒绝、权限应用、提交给 provider，以及完成、失败、未知、过期。原提案和事件禁止 UPDATE/DELETE；终态不能重新进入等待。主人决定先持久化设备及 generation，再通知原始等待者。
- Codex 原生命令严格匹配原 item ID、命令、完成状态和退出码；文件修改匹配原 changes；MCP 匹配 server、tool 和 arguments。只有匹配回执可记完成。失联、停止或没有匹配回执保持未知，读历史不会重发授权。
- 本地受限文件工具另记实际文件摘要/nonce/重放事实。Claude/Octos 记录决定、权限应用与发送确认；尚无逐工具结果关联，因此不凭整轮回复推定其工具完成。
- 模型请求与其持久回复同一事务结算审计；普通工具不能因模型回复成功而变成已完成。无 provider 通知及停止结果不产生模型完成证据。注入审计写失败时，回复与审计全部回滚。
- 原生 My Agents → Approval history 提供 100 条分页、原始提案、时间线和完整提案复制；预览限制 16,384 个 Unicode 字符。Agent 和所选记录校验阻止旧响应覆盖。未知 phase 显示未知。中英文提示区分主人允许与执行完成。
- `NativeOwner::AgentHistory` 仅在当前 SDK/主人/设备身份下进程内使用，不增加公开 HTTP 入口。无本地运行实例时未闭合授权保守记为未知，表示本设备未观察到运行，并不证明外部副作用不存在。已停止或恢复出来的历史不能进入审批队列。
- 完整账本备份含审计表和事件。兼容 A7 初版和 A5 用量版快照：保留原有费用、时间和预留，只迁移空审计表，不编造历史决定。

## 已运行验证

| 验证 | 结果与证据 |
| --- | --- |
| Agent 完整测试 | 106 库测试、3 programming、8 runtime 通过；3 项既有 ignored。`/tmp/hagency-audit-local-release.log`。之后新增的 A5 快照兼容测试单独通过：`/tmp/hagency-audit-legacy-usage.log`。 |
| 引擎库完整测试 | 251 通过，7 项既有 ignored。`/tmp/hagency-audit-engine-release.log`。 |
| 桌面库完整测试 | 615 通过，2 项既有 ignored，含实际注册控件的旧响应隔离、长中文预览与完成状态验证。`/tmp/hagency-audit-app-release.log`。 |
| 原生主人真实 HTTP/OAuth/磁盘链 | `native_in_process_facade_pins_matrix_account_without_exposing_credentials` 通过；备份恢复保留原设备决定和原命令，停止后未知、跨 Agent 详情拒绝。`/tmp/hagency-audit-native-release.log`。 |
| 真实已安装 Codex 模型 | 使用当前本地登录，在一次性工作区修改 existing.txt、运行 shell 创建 command.txt、显式 MCP 创建 mcp.txt；逐字节读回正确。5/5 工具返回，5 条独立审计均有 OwnerAllowed/Submitted/Completed，含命令、原生 file_change 和 MCP。整进程树停止成功。input 16588/output 246。`/tmp/hagency-audit-actual-codex.log`。 |
| 静态检查 | 严格三包 all-targets no-deps Clippy 通过：`/tmp/hagency-audit-clippy-release-3.log`；hagency/agent-local 格式、空白检查通过；i18n 2445 条/1841 调用/0 缺失；生产调用入口检查 wired 1/1：`/tmp/hagency-audit-callers.log`。 |

真实模型复现：

```sh
CARGO_TARGET_DIR=/Volumes/Data/Works/hagency-org/hagency-client/target \
cargo run --locked -p hagency-agent-local --example codex_programming_handshake -- \
  /Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex \
  /Users/chris/.codex
```

该示例的第二个参数使用 provider 自身的本地登录，不读取或复制登录文件；不传第二个参数只做无模型费用的握手。审批人为本地测试主人，不是远程 Matrix 人工裁决。

## 保留的验收与范围

本批没有完成原生窗口人工操作、真实加密 Matrix 远程审批、跨设备协作或 Windows/Linux 实机验收；自动回归和本地模型证据不替代这些门槛。远端 CI 按提交分别记录；排队、取消和运行中不算通过。最初 Clippy 找到两个 let-and-return 风格问题和测试模块位置问题，改为循环尾表达式并将测试移到文件末尾，未放宽警告规则。

下一批先完成 A6 停止请求检查与显式主人处置，再按原计划实施 A2 文件收发、A4 加密执行和远程审批、A3 多 Agent 协作。未知原请求及其已发生的费用、文件和回执持续保留。

远端 A1 PR9 的 `732c99d5`：Windows 构建成功，Linux/macOS native 通过；Full matrix 的 Windows 作业仅 `hagency-execution --lib` 中既有 `native_owned_approval_caller_loss` 失败，40 通过/1 失败。原日志在 approval_loss.rs:210 的通知流提前关闭后 unwrap(None)；该文件与 main 相同，现有日志没有 fault 轮次或原始停止报告，原因仍未判定。保留失败，后续先补原始诊断再修复，不能用本机成功替代 Windows 结果。完整日志 `/tmp/hagency-pr9-windows-732c99d5.log`。
