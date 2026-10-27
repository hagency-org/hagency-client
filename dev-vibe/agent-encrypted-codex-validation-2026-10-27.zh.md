# 主人加密任务 → Codex → 加密回复与文件：端到端验收

主人与 Agent 的私聊完整链路已实现，并通过真实端到端验证。本批交付运行链，
不再以身份、解密或 ACK 模块单独作为验收终点。共享 Project 的其他请求者加密
执行尚未开放；本文不宣称所有平台、生产 OAuth 或人工桌面操作均已验收。

## 使用路径

1. 在当前执行设备配置本地 Codex，选择 Agent 工作区，启用编程执行和服务。
2. 用已验证的主人 SDK 设备，核对并验证该 Agent 的独立身份；缺少私人签名密钥
   时按现有恢复提示处理，不以缓存的 verified 布尔值放行。
3. 新建的主人 Agent 私聊默认启用 Megolm；旧的明文私聊仍兼容，可以显式升级。
4. 在该私聊发送加密任务或加密附件。已有预算、工具策略和精确审批继续生效。
   Agent 可调用 receive_file、实际命令/文件修改、send_file/get_file_delivery，
   用自己的 Matrix 身份发送加密文件和回复。

## 完整实现

- 现有 NativeOwner 服务轮询原始 AS 密文、取得实际 SDK 证明并原子保存本地输入。
  现有任务循环直接消费该记录；独立 encrypted Start 和新开始回执才产生一次运行
  许可。每次工具重新核对原始密文历史承诺、执行 ID、设备、Room 和当前租约。
- 复用实际 Codex 编程适配器、三层策略、费用账本、审批审计和整棵进程树停止。
  明文协议继续拒绝 encrypted_* 记录，不将密文 JSON 当作模型提示词。
- 附件声明来自实际解密的原消息。原 SDK 认证下载、v2 校验解密和原子落盘继续
  绑定原请求。发送仍先冻结真实文件和审批，绝对路径只能规范为该工作区的原始
  子路径；目录、symlink、替换文件和越界检查保留。
- 加密文件在本机暂存，由主人的实际 SDK 加密后上传；实际 HTTP 队列处再次检查
  原始存活条件并禁用重试。文件名、私钥和解密密钥只进入加密消息，不交给 Server。
  暂存状态标识为执行主机观察，不能冒充 Server 上传或投递回执。
- Agent SDK 为已验证主人设备创建单事件独立 Megolm 密钥和加密 key-share；
  Server 的独立发布事务在网络前永久提交 possible。只有首次接收交出发送工作，
  相同原 ID/原载荷仅查询原状态，改变载荷冲突；未知不重新上传、不换消息发送。
  每次 key-share 和 Room 写入前复核当前执行。实际 HTTP 原回执才记 sent。
- 回复和文件分开记录；文件 sent 不等于任务回复完成。原回复在实际 provider
  整树停止后发布，真实回执使原执行进入 encrypted_completed。
- Receive broker 只新增当前永久主人设备的 signed_curve25519 key claim，仍拒绝
  任意 peer、Room/media/to-device 写入。AS 凭据继续只在 Server，SDK未升级到 main。
- 主人完整账本快照保留原始加密输出见证；恢复不会重新封装或重新授权发送。

## 真实端到端结果

当前 Server/PostgreSQL/AS/workers、真实 Synapse、桌面 SDK、NativeOwner 与真实
Codex 的成功证据位于：
`/private/tmp/hagency-encrypted-codex-20261027-2/flow1/attempt2`。
OAuth introspection 是明确的隔离测试桥接，底层 Matrix 身份由真实 whoami 核对；
这不替代生产 OAuth 验收。

- 输入为真实加密 m.file：131073 字节。Codex 实际接收、检查并运行命令，生成
  131097 字节文件，内容是原始字节加 `\nENCRYPTED_CODEX_OUTPUT\n`。
- 主人的实际 SDK 收到并解密生成文件；与工作区真实文件和预期字节逐字节一致。
- 原加密输入：`$bKgne36McvIanFtU-OLCICcrc_YeCKITRvQBlvnExYk`。
- Agent 原加密文件：`$5WJc3mzzpqGjuTt7oEdCkwNAjpT_n9yyQ7poHbx5ClI`。
- Agent 原加密回复：`$7XtJNoXKXnz_qE6I-9Rf2RUcFf4niN_lvAAZ3UrrN9c`。
- receive_file、实际命令和 send_file 的原审计均 completed；原停止记录为 whole_tree。
- 成功任务一调用，input44629/output467，共45096 Token；cachedInput28672、
  reasoningOutput15 为子集，不重复加总。pending/unreported/reserved 均为0。
- 两次 NativeOwner/原服务重开后，成功任务仍只有一调用，用量完全相同；原密文
  仍可解密，无模型重跑。另一次正常创建验证新私聊默认加密且能够激活。

独立 Python 读取实际 homeserver、PG 和 SQLite，核对原始 source/transport 摘要、
SDK 生成的原密文与真实事件正文、原执行 ID、费用、三类工具审计和文件摘要。
`independent-proof.json` 证明成功请求只有2个实际加密发布，Server 明文文件
outbox 行0，原 AS 正文已压缩但来源密文保留；当前活租约0、历史租约行2保留。
`restart-public.json` 保留重开前后的一调用用量与默认加密私聊实测。

## 失败与费用保留

- 初始驱动误传模型配置的 binding 参数、旧私聊登记/激活中的明文限制和 fixture
  重开辅助步骤均在推理前暴露并修复；没有作为成功计数。
- 第一环境真实模型创建了文件，但 send_file 的绝对路径被拒绝；原失败回复、
  整树停止及32725 Token 保留。修复只接受工作区内绝对子路径，未放宽文件边界。
- 该测试容器随后消失；原数据仍在。恢复后的 AS 事务1与已保存原内容冲突，Server
  拒绝409。保留失败与数据，未修改事务摘要或伪造 ACK；后续改用独立的新环境。
- 第二环境首个任务遇到 Server 未接受实际 SDK 的 org.matrix.msgid 元数据。
  原加密上传、未知发送记录、失败原文和117460 Token 全保留（超过该测试 Agent
  的估算预算亦按实记账）。修复仅允许 SDK 的有界 msgid，不接受任意明文字段。
- 最终成功使用新的独立 Agent/原始任务；未重新调用、抹掉或重发上述失败任务。
  本批三次实际推理合计195281 Token；45096仅为成功任务的费用。
- 中间编译、旧快照夹具、Clippy、普通 PG 命令的忽略项及独立脚本表名错误也保留；
  结论只引用修正后的完整检查，不把先前失败改写成通过。

## 最终自动检查

Agent-local170通过/3原有忽略；Core284通过/9原有忽略；App628通过/2原有忽略。
Server 新鲜专用 PG 显式 include-ignored 全量125通过/0忽略。SDK4项实际密码学/
HTTP 集成及严格库 Clippy 通过。其余最终 lint、绑定与远端记录见本批 progress。

本次完成的产品范围是主人私聊的完整加密任务/编程/回复/文件收发。共享 Project
请求者信任、跨主机密钥恢复、人工桌面/生产 OAuth 与其他平台实机资格继续在总
任务清单中保留，不能由本次 macOS 端到端结果替代。
