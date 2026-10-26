# Agent 文件收发：当前实现和验收

2026年10月27日。A2 进行中。本页保留第一批附件描述协议证据；当前已进一步接通接收工具，见 [原请求附件接收验收](agent-file-receive-validation-2026-10-27.zh.md)。下述“尚未启用”描述是第一批提交的历史状态。
本功能分支 `codex/agent-file-transfer` 基于 A1/A7/A5/A6 实现；已重新拉取并确认
包含最新 Client main `42e59b9e`。配套 Server 基于 main `6340df32`，第一批提交
`c98e210`，详见其 `dev-vibe/agent-file-metadata-contract-2026-10-27.zh.md`。

## 当前可验证行为

- Server 从标准 file/image/audio/video 原始事件保存附件声明，和原请求原子
  入库；同一事件改变声明会拒绝，附件描述禁止改写或删除。
- `PollFiles` 明确发送 `acceptFiles:true`，原 `Poll` 不发送该字段，仍只取文本。
  当前运行链继续使用原 Poll，待收发工具完整接通后才启用附件请求。
- 新 `Attachments` 宿主操作只能指定原 lease/dispatch/execution。Server 从其
  自己的原事件和当前 Gateway 状态取得附件，没有任意 Room/event/URL 参数。
  旧 Server 的不支持错误不能被当作成功或制造空文件。
- Client 严格校验回传执行编号、最多一个附件、声明摘要及 MXC/v2 加密参数。
  Debug 隐去 source；模型可见 Summary 只有文件名、类型、声明大小、摘要。
  私有 MXC 和解密 key 不能进入模型工具结果。文件名及多行 caption 都是不可信
  声明，不能作为落盘路径或已验证内容。
- 两仓库保存相同的纯明文与加密合成向量。摘要使用递归排序键的紧凑 UTF-8
  JSON，避免两端 serde_json preserve_order 不同导致摘要不兼容。该摘要不是
  下载后明文哈希，也不是投递或执行成功的凭据。

## 已运行检查

- 当前 Agent-local 库116通过/3个原有忽略；engine库255通过/7个原有忽略。
  Agent-local programming3、runtime8通过。旧忽略项保持原状态。
- 新附件描述3项、设备请求/响应2项通过；真实 NativeOwner OAuth/设备认证/
  TCP typed transport 额外1项通过，验证精确请求、执行错配和源替换拒绝。
- 两包严格 all-targets no-deps Clippy、格式、production-caller 检查与 whitespace
  校验通过。没有新增 GUI 字符串或桌面交互变更。
- Server 最终在每轮新建的独立 PG 测试库串行跑完整95项，0失败/0忽略；
  OpenAPI78个 owner及4个AS操作、15个负向漂移守卫、严格Clippy与格式通过。

日志：`/tmp/hagency-files-client-{libraries,runtime,tcp,wire,descriptors,clippy,fmt-check,callers}.log`；
Server：`/tmp/hagency-files-server-{serial-final,attachment-final,openapi,clippy}.log`。

较宽的 `cargo test -p hagency-agent-local -p hagency` 在现有
`native_two_agent_qualification_records_its_evidence` 检查处失败：仓库记录仍是
未进行真实双 Agent 资格验收的占位内容。原失败保留于
`/tmp/hagency-files-client-all.log`，不能据本批元数据测试宣称全量测试或双 Agent
验收通过。Server 初次误用并行数据库测试、随后复用旧数据库的失败也保留，
具体锁和分页原因见配套 Server 报告；生产逻辑没有增加失败重试。

A1另行远端检查：`1724b7d9` 的 run40363073237 中 Linux 和 Windows native
任务通过，macOS取消，因此整体不是全绿；这也不证明先前 caller-loss 失败
的原始原因。A6 `c5f42926` 的 CI通过、全矩阵和App CI尚在运行。

## A2 完成前仍须交付

1. 在当前宿主运行链、原执行和完整三层策略中接通 list_received_files 和
   receive_file，精确主人审批、认证有界下载、加密附件校验和私有生成路径落盘。
2. send_file 冻结实际文件字节/大小/明文哈希，绑定原 Room/thread；持久化发送
   意图、唯一 custody、上传与固定 Matrix txn，提供 get_file_delivery 状态。
3. 验证实际字节、授权中途撤销、已存在目标、未知写入/上传/发送、重启和备份
   恢复后的重复调用；unknown 不自动重发。模型等待只读状态，不把 pending
   或 unknown 当作已投递。
4. 真实当前 Agent、桌面和跨平台联合验收；与 A4 接通加密 Agent Room、远程
   私密审批的完整路径。当前明文元数据路由不能替代 A4。

本批没有真实文件下载/发送、生产消息、部署或 A2 完成声明。总体 A2-A7 目标仍在进行。
