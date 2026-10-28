# 原始 AS 密文队列与租约限定读取

本批继续 A4，将真实加密事件从 AS 原事务保留到执行设备可读的独立队列。
目标仍是完整加密任务执行/回复/文件与 A3；本批不把读取和解密当成任务 ACK、
准入或模型执行，也没有移除普通明文路径的加密 Room 拒绝条件。

## 实现

- AS HTTP 确认仍只表示原事务与路由意图已持久化。路由复制规范化原密文到
  ciphertext_inbox 后，才可压缩原事务正文；存储失败或队列满保留原路由意图。
  存入操作在同一事务锁中核对 pending AS 原事务确实包含该完整 raw event，
  同时核对当前 Room/member/binding/Agent 事实。没有接受 caller 提供的“AS 证明”。
- 记录原 transaction、binding/Agent generation、事件/房间/发送者、服务端接收
  时间及 SHA256承诺。摘要使用排序 UTF-8 JSON `["agent-encrypted-inbox-v1",event]`，
  不依赖 serde_json preserve_order。只保留 encrypted event 的实际六类字段。
  标准外部 m.relates_to 作为不可信元数据保留，路由不据此推断 thread/mention。
- 相同 binding/event 幂等，密文改变冲突。普通 Poll/附件 Poll 不读取该表；已知
  Agent 发送者被忽略，防止加密回复造成循环。容量与普通待办共享主人上限。
  过期可清除密文，但保留原身份/摘要墓碑；DB trigger 拒绝改写、删除和重新激活。
- `agent-encrypted-inbox-v1` 新增两个 device-only POST：ciphertexts/poll 与 read。
  前者必须给出当前 lease/binding，最多4项；后者只接受原 inbox ID 与 lease。
  均不接受 URL、消息原文、调用方成员事实或可信标志。读取逐项取得实际 Gateway
  事实，并重新核对 owner/session/device/assignment/lease、两类 generation、
  暂停、过期、当前成员及加密状态。它没有推进 owner_events/execution 状态。
- Client typed response 核对当前请求的 Agent/binding/原 inbox ID、头部与原事件一致、
  规范化摘要和大小。数据 DTO 可复制，但不携带准入或执行权限。
- Native host-only `decrypt_agent_ciphertext` 只接受 lease 和原 ID，主动读取原件；
  由原 Agent SDK 验证主人设备并解密后，再次读取原件并核对当前权限与摘要。
  返回不可 Clone/Serde 的 `DecryptedAgentCiphertext`。它和裸密文解密证明是不同
  类型，不能用 caller JSON/任意 Room 读取代替原 AS 来源。当前 receiver 仍只
  信任永久主人；其他请求者验证与完整模型路由继续实施。
- 原始读取/解密/回读由同一共享任务持有更新屏障。停止按 Agent 等待原任务；
  丢弃 observer 不丢原工作。审查修复了 owned job 获取 Native calls 锁的潜在
  死锁：Stop 可能持该锁等待任务，因此内部任务只用各自 SDK/source 同步锁，
  不嵌套调用会获取 calls 的裸事件入口。

## 验证

- 新鲜 PG 全量122项通过，无 ignore；最终3项 targeted 回归还覆盖实际原 AS
  source、压缩/重开、错误设备/当前成员/加密状态、内容冲突、容量、过期墓碑及
  Agent 回复忽略。普通队列保持为空。严格 all-target Clippy通过。
- Client281项通过、9项既有 ignore，包含2项新增 typed wire/选择器负例；
  同时核对另一个 binding/inbox response 不可替换原请求。严格 no-deps all-target
  Clippy通过；相关 App 和显式资格驱动均编译通过。未重复宣称本批 UI人工验收。
- 两端使用同一字节 fixture，分别匹配独立 Python 计算的 SHA256。OpenAPI
  100owner+4AS操作、27个负例通过。Rust spec1201绑定无 missing/deferred/platformDeferred，46项守卫及生产调用检查1通过。
- intake1 在第1套全新 Synapse/PG/SDK 中通过原 AS→队列→当前 lease→Native
  解密→回读，以及错误 Agent/epoch/released lease 拒绝。独立 SQL证实 AS 已
  routed/正文已压缩，原密文及摘要保留。它是停止锁修正前、单消息的成功证据。
- 最终 intake2 在第2套全新环境验证上述链路，并包含真实 encrypted thread。
  普通消息和线程均经过 Owner SDK 实际加密发送、真实 AS、保留队列、Native
  Agent SDK 解密和当前 scope 回读。错误 Agent/epoch/释放租约仍拒绝；原生目录
  重启1→2后原密钥/信任/Megolm保留。日志原进程 exit0。
- 独立 Python 分别查询实际 homeserver 的原事件，逐字段比较两条保留密文，并
  重算两条规范化摘要。SQL证实 AS均已 routed/正文压缩、该 binding 的普通
  owner_events0、原生账本模型调用0、历史 lease1/current live lease0。保留
  历史 lease，不能说成 lease表没有行。没有付费推理。

最终证据：`/private/tmp/hagency-agent-ciphertext-intake-20261027-2/intake2/`
的 result.json、as-inbox-public.json、as-thread-inbox-public.json、
independent-thread-event-public.json 与 independent-as-custody-proof.json。
日志 `/tmp/hagency-ciphertext-proof2.log`。第一套证据另行保留。

保留最初 borrow-check E0505/增量 rustc ICE 及驱动编译失败；修复借用后用
CARGO_INCREMENTAL=0 验证，未清理共享缓存或改生产编译参数。SDK reqwest 未开
json feature，驱动改为显式 JSON bytes，没有扩大依赖或伪造响应。失败日志未改写。

## 继续完成的链路

下一步把新的原 AS/租约绑定证明消费到主人本地持久 inbox，保存原密文承诺与
实际解密内容的对应关系，再发送精确任务 ACK/准入；不向 Server 发送明文正文。
必须将服务端不可变密文摘要与本地 history/去重/重开校验接通，不能以旧纯文本
摘要冒充它。随后接原 Codex 执行、每次工具当前权限核验、Agent SDK 的加密回复
和文件一次性投递；共享 Room 的 requester 信任/mention/thread 仍需完整验证。
完成后才移除相应加密执行拒绝，再接 A3 和原有人工/平台/生产OAuth门槛。

两套受控 Synapse/Server 与专用 PG 均已停止，保留原数据。未部署或合并。
