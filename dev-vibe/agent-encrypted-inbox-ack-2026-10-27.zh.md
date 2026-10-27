# 原始加密输入本地持久化与 ACK

本批在现有 AS 密文收件箱基础上，接通 NativeOwner 的原始输入接收。
这份报告记录输入保存和确认，不代表完整加密 Codex 执行通过。

## 实现

- Server `agent-encrypted-admission-v1` 提供 ciphertexts/prepare、ack、admission。
  只接收原始 inboxId/sourceDigest、当前 lease，以及主机实际 SDK 解密得到的
  thread/mentions 投影。每步重新验证原始来源、主人、设备、成员、绑定和代次。
- Server 历史正文保存规范化原始密文；独立 encrypted_* 状态和数据库约束阻止
  普通 Poll/ACK/Start 使用密文正文。准备/ACK 幂等，换租约保留同一 dispatch。
- Native 消费真实 SDK 与原始 AS 证据，在主人本地账本的一次事务中写入明文、
  原始密文、来源摘要和两种历史摘要，成功后才发 ACK。原始异步工作由主机持有；
  retirement 在授权前和 HTTP 前检查，Stop 继续等待已开始的原始工作。
- 本地历史核对使用 Server 密文摘要，同时保留明文摘要；快照保存完整关联。
  通用模型准备/开始继续拒绝加密输入，ACK 本身不产生执行许可。
- 对 Agent/Room/session/密文字节计算独立 seal 摘要，规范化 Base64 padding。
  相同加密载荷换 event_id/时间不得产生第二份工作；Server 和本地均保留唯一约束。
- Matrix SDK 专用无 Room 接收入口先移除外层未经加密认证的 m.relates_to，保留
  解密后真正的关系。主机拒绝仅有外层关系的消息，避免伪造线程被解释为任务路由。
  SDK 9b547c0e 延续原0.19.1 history 分支，未升级到 SDK main。

## 真实联合证据

隔离 fixture `/private/tmp/hagency-encrypted-ack-20261027-1/ack1`，原始日志
`/tmp/hagency-encrypted-ack-live1.log`。实际 Synapse、正常创建的两个 Agent、
主人 Matrix SDK 和 NativeOwner 接通；普通输入和真正加密线程均先保存再 ACK。
重复接收、原生授权1→2重启以及新租约保留原 dispatch ID 和原始密钥。
伪造外层线程、同一密文换新 Matrix event、错误 Agent/epoch/Room/sender、篡改
密文和已释放租约均被拒绝。导出和 Inspect 实际主人账本快照成功。

独立 Python 经实际 homeserver 读取原事件，并读取 PG 与本地 SQLite；校验
原始 source、seal、Server 历史 transport 和本地 plaintext 摘要逐字节一致。
`independent-proof.json` 证明原始密文3份、admission2份、普通明文事件0；
3份 AS 原始事务正文已压缩，密文仍保留。本地模型调用0、当前活租约0、历史
租约行1。释放租约后的2份未执行任务为 encrypted_pending，历史并未删除。
本批无付费推理。

## 自动检查与限制

- Agent-local 全量167通过/3原有忽略；Core283通过/9原有忽略。
- Server 新鲜专用 PG 全量123通过/0忽略；严格 all-target Clippy 通过。
- OpenAPI 103 Owner +4 AS 操作、30项负例通过。
- SDK 专用解密集成2项和严格库 Clippy 通过。
- Client affected all-target 严格 no-deps Clippy、App628通过/2原有忽略、
  Rust1208绑定无 missing/deferred/platformDeferred、46项静态守卫及调用审计通过。
- Core/Agent-local 与 App 改动文件格式通过；所有本批 fixture 服务和 PG 已停止。

保留初次测试私有辅助函数访问错误、旧快照夹具未移除新表、seal 字段迭代期间
测试错误和两次 Clippy 错误；修正后复测。Server 最初普通 test 命令实际仅运行
35项/88忽略；随后显式 include-ignored 在专用 PG 运行123项，以上全量结论只
引用后者。独立 Python 首次误把本地序列化字符串 hash 当作裸字节 hash，已按
真实账本编码修正；未更改产品摘要规则。

下一步是独立加密模型/工具 Start、原始加密回复与文件收发、共享请求者信任和
A3 协作；人工桌面、生产 OAuth 和各平台资格仍保留。先前744496fd的四条 CI
全部成功，仅代表前一提交，不能代替本批新增源码验证。
