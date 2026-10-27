# Agent 接收主人加密事件

本批推进 A4 的密码学接收链。原生桌面正常创建/启动 Agent 会打开独立接收 SDK，
由主人当前已验证 SDK 的实际公钥建立主人信任。完整目标保持进行中：本批不授予
入站任务接纳、ACK、租约或模型执行权限，既有加密 Room 执行拒绝仍保留。

## 实现边界

- Server 新增 `agent-crypto-owner-receive-v1` 独立命名空间8个操作；原身份接口
  仍为7个。原永久公钥、当前 owner/session/device/Agent generation 与分配校验复用。
  接收模式转换保留同一个 SDK store/设备密钥，不因 URL 或授权代次生成新身份。
- 查询限于本 Agent 与永久主人；给主人 master key 签名必须使用已固定的本 Agent
  user-signing 公钥。转发只保留该 Agent 的签名，不代为发布其他用户签名。
- sync 强制当前 Agent/device、零等待、离线 presence、空 Room/presence/account-data
  过滤器；调用方不能提供 filter 或替换身份。接受 SDK 固定附带的
  `use_state_after=true` 兼容标记，不因此开放 state。成功响应最多1MiB/256个
  to-device 事件，任何 Room 内容均拒绝；无 room history、key claim、发送和备份接口。
- Agent 只信任当前实际主人 SDK 已验证且当前设备经交叉签名的 master key。
  重新查询验证后，用 Agent 自己的 user-signing key 签署主人根，随后接收实际
  Olm/Megolm to-device 密钥。主人根或 SDK owner/device/origin 改变必须拒绝。
- SDK 固定到 `fc640f9e`，新增只使用原本地 crypto store 的无 Room 缓存解密入口。
  缺少密钥直接失败，不查历史、备份或发起密钥恢复；普通 Room API 保持原行为。
- 原生 `decrypt_agent_owner_event` 是 host-only 方法，不加入序列化管理命令。
  返回不可 Clone/Serialize/Debug 的 `DecryptedAgentEvent`，明文只经消费方法取出。
  验证实际密码学发送设备、主人 root、设备签名、非 forwarded keys 及消息类型。
  外部 event ID/room/header/digest 仍须由未来原始 AS 密文接纳绑定；此对象单独不证明
  事件来源、当前房间成员资格或执行许可，不可代替 Server admission/lease。
- 原始解密任务持有更新屏障和独立关闭凭据。取消等待者不取消原任务；停止按 Agent
  等待接收任务，完整 shutdown 等待全部原任务。任务登记与停止快照在同一 slot 锁下
  串行化，防止停止漏掉已准入任务。最多32个未完成任务。

## 当前验证

- Server 新鲜 PG 全量119项通过（无 ignore），严格 all-target Clippy通过；
  OpenAPI98个 owner操作与4个AS操作、24项静态负例通过。
- Native 全量279项通过、9项既有 ignore；App628项通过、2项既有 ignore。
  停止等待补强后，8项针对性 Agent crypto 回归通过。
- Client `--no-deps` all-target 严格 Clippy通过。包含依赖 lint 的尝试被原有
  article-makepad27项告警阻止，失败日志单独保留，没有修改或屏蔽该依赖。
- Rust spec1199绑定无 missing/deferred/platformDeferred；46项守卫及生产调用检查通过。
- 第4套全新 Synapse/PG 的 receive5 已实际通过：正常 Native CreateAgent 自动准备两个
  独立接收器、主人/Agent 双向交叉签名、正常 owner-direct 创建/加入、主人真实 Megolm
  send、读取原密文并经 Native 不透明证明得到精确正文。另一个 Agent、错误 generation/
  room/sender 与被篡改密文均拒绝。NativeOwner 原目录重启，设备授权1→2，原 Agent
  keys/信任/Megolm密钥保留，原事件仍解密成功，停止后的 host 拒绝继续读取。
- 独立 HTTP 查询与 OpenSSL 验证实际 Agent user-signing key 对主人 master key 的
  Ed25519 签名；数据库只读核验原生账本 calls0、Server leases0/public-pins2。
  成功证据位于第4套环境的 receive5/result.json、ciphertext-public.json、
  independent-owner-signature-proof.json 与 independent-accounting-proof.json。
  `/tmp/hagency-receive-proof5.log` 原进程 exit0。没有触发模型执行。
- 初次独立签名核验使用 owner 身份查询，协议没有返回另一个用户的 user-signing
  key，因此核验未运行；改用隔离测试 AS 以该 Agent 查询自己的实际公钥后，OpenSSL
  原签名验证成功。没有更改 Matrix 权限或向产品提供 AS token。

## 保留的失败运行

- receive1：严格参数校验拒绝 SDK 自带的 `use_state_after=true`，接收器未就绪。
- receive2：尝试在同一 Synapse/device 使用全新 owner SDK store，真实 OTK 冲突拒绝。
  保留该错误；后续改用全新 Synapse/PG/SDK 环境，不绕过原设备密钥约束。
- receive3：分阶段错误确认上述 SDK sync 参数问题；主人查询/签名已经通过。
- receive4：参数兼容修正后两个接收器已就绪，但测试服务缺少正式后端使用的
  identity/membership cleanup worker，新 Agent 保持 creating。已在显式测试服务
  启动相同 worker，驱动只观察同一 owner-direct journal，不重建未知操作。

路径位于 `/private/tmp/hagency-agent-crypto-receive-20261027-{1,2,3,4}`；
`/tmp/hagency-receive-proof{1,2,3,4}.log` 保留各自原结果。没有付费推理。4套隔离 Synapse、对应 Server 与专用 PG 均已停止，原数据保留。

## 后续完整执行

继续接原始 AS 密文入库与 ACK、当前 binding/成员/执行设备/租约的实际核验、
原本地任务一次性接纳、Codex 输入及原加密回复/文件投递，再完成 A3 多 Agent 协作。
生产 OAuth、人工桌面和原有平台验收仍需独立完成。总目标未完成，不部署或合并。
