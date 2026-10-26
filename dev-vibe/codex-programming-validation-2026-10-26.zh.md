# Codex 编程执行验收

Client 按 [Codex app-server 协议](https://learn.chatgpt.com/docs/app-server)在当前主人 Agent 运行链增加了显式 Codex 编程模式：原生命令、已有文件修改和主人配置的 MCP 使用原有三层工具策略与精确审批。默认聊天和受限 host files 模式继续独立运行。

分支 `codex/codex-programming` 从已拉取的远端 main `42e59b9e` 创建。修改位于独立工作树；原源码树已有的未提交记录保留。

## 使用方式

在执行设备打开 **Agent → 模型与资源 → 启用 Codex 编程执行**，填写 MCP JSON 后保存模型并重启服务。Agent、Room 和请求者三层高风险策略都需要允许该操作或询问主人；任意拒绝仍生效。

MCP 支持显式 stdio 和 HTTP 配置。Stdio 使用真实绝对可执行文件路径和参数；HTTP 使用 HTTPS 或本机 loopback HTTP。MCP 固定在本地执行环境，每次工具调用仍通过审批。不会导入个人 Codex 的服务器、鉴权头、环境注入和工具免审批配置。编程模式拒绝已有个人或项目 `.codex/rules` 目录，避免其 prefix 审批绕过主人策略。

编程目录由 Agent 选择，供其 Room 共用，不宣称不同 Room 的操作系统级文件保密隔离。MCP 和编程设置保存在执行设备；旧版远程模型配置不导出这些设置，修改普通 Codex 模型时保留它们。没有修改 Server 或恢复旧 Fleet。

## 实现

- 本地模型配置增加默认关闭的 programming 字段和 SQLite 增量列；保留身份、预算及未决记录。
- Codex 使用 guardian 或 Windows Job 管理原始进程树，覆盖正常停止、初始化取消和 owner Drop。
- 命令、文件修改和 MCP 提案绑定当前作用域与原始执行；审批前后使用真实主人、设备、租约及运行请求校验。只返回单次接受，不写永久 exec 或网络批准规则。
- 编程上下文和累计计费基线独立于聊天及受限文件模式，身份包含 MCP 配置、工作目录、模型、推理档位与搜索设置。未知执行不重跑，缺失用量不把历史累计值当作本次费用。
- 桌面支持设置、重读、无效 JSON 提示、运行能力说明及中英文文案。

实际安装程序暴露了两个仅打开 feature flag 无法解决的问题：CLI 的 disable 参数会覆盖普通配置覆盖值；旧聊天链的 `environments: []` 会禁用执行环境。编程模式现在显式 enable shell/unified_exec，并保留默认本地执行环境。协议回归检查这些启动参数。

## 本机验证

环境：macOS、Rust 1.98.1、Codex CLI 0.164.0-alpha.11.1。

| 检查 | 结果 |
| --- | --- |
| hagency-agent-local 完整测试 | 84 个库测试、3 个配置与进程测试、8 个跨运行时测试通过；3 个原有忽略项 |
| hagency 引擎库 | 250 通过；7 个原有忽略项 |
| hagency-app 完整库 | 610 通过；2 个原有忽略项 |
| 原生管理 UI 定向回归 | 54 通过，包含编程设置保存、重读和远程设备隔离 |
| 三包所有 targets 严格 no-deps Clippy | 通过 |
| 引擎与本地 Agent rustfmt、diff whitespace | 通过 |
| 中英文文案 | 2376 条，1791 个调用，0 缺失 |
| 实际安装 Codex 的无推理探针 | shell 功能、显式 MCP 启动、独立上下文恢复、整树停止通过 |
| 真实模型编程 | 见下文 |

真实模型在临时工作目录将 existing.txt 从 `before\n` 改为 `after\n`，通过原生 shell 创建内容为 `command-ok\n` 的 command.txt，调用显式 MCP 生成 `mcp-ok\n` 的 mcp.txt，并读回验证后回复 ACCEPTANCE_COMPLETE。工具返回/请求为 6/6，涉及原生命令、文件修改和 MCP 的精确测试主人审批，结束后取得整树停止结果。记录 input 21632、output 352；该轮实际输入超过单次预留估算，按实际用量记账。

该模型验收使用当前适配器与测试主人审批队列，复用本机 provider 自有登录，不读取或复制凭据；它不是生产 Matrix 账号端到端或桌面人工操作验收。Windows/Linux 实机和生产部署未在本机证明，远端 CI 状态见 PR。

主要日志位于 `/tmp/hagency-programming-{local-publish,engine-final,app-final,clippy-final,handshake-release,real-model-5}.log`。探针命令：

```sh
cargo run --locked -p hagency-agent-local --example codex_programming_handshake -- /absolute/codex
```

传入第二个现有 Codex 登录目录参数会启动真实模型并产生 provider 用量；不传时只进行无推理探针。

## 远端 CI 跟进

PR #9 首轮 Windows 严格 Clippy 发现 `codex::process::Child` 的 Direct
分支在 Windows 达到 272 字节，触发 `large_enum_variant`。现在使用 Box
保存直接子进程句柄，保留原有停止与 Drop 行为。修正后的本地完整
hagency-agent-local 测试及所有 targets 严格 Clippy、rustfmt 均通过；
Windows 的修正结果以新提交对应的远端 CI 为准。

本轮再次执行 `git fetch origin main`，远端仍为 `42e59b9e`，也是分支的
merge-base。未在 main 上直接修改。
