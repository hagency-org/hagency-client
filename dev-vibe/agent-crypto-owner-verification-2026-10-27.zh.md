# 主人验证精确 Agent 身份

本批继续 A4 加密执行链，接通主人 Matrix 账户对 Agent 主密钥/设备指纹的实际
交叉签名。它建立主人侧对 Agent 身份的信任，**不是**加密任务执行或 Server 租约
权限，也不代表 Agent 已验证主人/其他请求者。任务解密、原始加密回复/文件和
A3 协作仍继续实施；既有加密 Room 拒绝保持。没有付费模型推理。

## 产品入口与原生生命周期

- Service 身份区显示完整 Matrix user/device、Ed25519、Curve25519 和 master key。
  主人核对后可验证精确身份；勾选确认本身不改变信任。
- 原生 Verify 命令逐字段比较当前准备好的实际 SDK 身份和执行设备代次。任意
  字段变化均在签名前拒绝；带 `ownerVerified` 的身份 JSON 由严格 DTO 拒绝。
- 正常桌面 Source 使用当前主人自己的实际 Matrix SDK。主人账户须已验证，当前
  设备须经交叉签名；签署新 Agent 还必须持有私有 user-signing key，缺失时明确
  提示先恢复 Matrix 交叉签名密钥。不会在此流程中生成替代主人身份或复制私钥。
- 主人 SDK 从 homeserver 查询实际 Agent 身份，核对原 master/device 两类公钥及
  设备自签名/交叉签名链，使用自己的 user-signing key 签署精确 Agent master。
  签名写出后再次回读；只有实际签名链验证通过才显示已验证。另一个 Agent 不会
  顺带被验证；HTTP 成功、缓存 verified 标记或签名意图都不是证明。
- 重启保留原 Agent SDK keys，信任状态重新从真实主人 SDK/Matrix 公钥签名观察。
  刷新失败或 transport 不可用时不展示缓存中的肯定验证结果。
- 每个 Agent 保留原验证任务的共享回执；重复点击共享同一在途任务。取消等待
  不丢掉它。停止与 checked shutdown 先退役权限再等待原任务；迟到的成功回调
  不能恢复旧会话。已经写出的 Matrix 签名保留为真实历史，不等于旧任务可继续。

## SDK 请求与新鲜度约束

SDK fork 当前固定到 `c1962ee2`，沿用原0.19.1历史分支：

- 单次请求可叠加 scope fence，保留 Client 的全局 fence。实际并发槽等待后、
  每次 HTTP 重试前重新核对；permit 持有到原 body 完成。作用域内请求不隐式
  刷新凭据，避免原生会话已停止后发生隐藏的凭据操作。
- 精确身份查询要求本次 keys/query 无部分失败，存在指定设备及完整的 master/
  self-signing/主人自己的 user-signing 公钥。比较本次响应与 SDK 验证后的密钥
  **和签名**；不把旧已验证缓存当成此次响应。无当前 master 返回 None；缺项、
  无效签名、无效设备密钥均拒绝。unsigned 展示注解不参与密钥证明。
- 独立 SDK 实际 HTTP 测试还保留普通未配置 scope 的兼容行为、全局 fence 不能
  被局部 fence 绕过、原 body 关闭等待，以及真实401不能触发隐式 refresh。

## 证据及失败记录

- SDK：1项 fence 单元、9项实际 HTTP 集成通过。新增响应完整性用实际 SDK
  生成并保留已验证缓存，再分别返回部分失败、缺 master/self-signing/
  user-signing/device、无效 self-signing 签名和无效 device key，证明不会借缓存
  通过。既有 required encryption3项、独立 own-device Megolm2项回归通过。
- Native：原有5项生命周期测试与新精确验证/停止等待测试共6项通过；新测试
  明确为 host double，不代替真实签名或 HTTP body 验证。桌面实际 Makepad 构建
  覆盖指纹确认、变更后清除确认、验证状态及其他执行设备禁用操作。
- `owner1`（第一套隔离环境）实际签名/回读及原生重启成功，随后源审查发现
  keys/query 部分成功时保留旧缓存的缺口；已补上述严格响应检查及负例。
  其成功记录不作为补强后最终源的验收。
- `owner2`（第二套全新环境）在新增“已交叉签名但缺私有 signer”的准备阶段失败：
  新 SDK 登录还未 sync/上传设备公钥，查询不到该设备，未执行这项负例。保留
  `/tmp/hagency-owner-verification-qualification2.log` 及原数据，不能记作通过。
  修复为使用真实 SDK sync 后再进行交叉签名，既有生产验证条件未放宽。
- 初始 SDK漏补内部 send_inner 调用参数、测试 MockBuilder priority 方法位置、
  RequestConfig feature-gated import、资格代码将 Option 当 Result，以及测试中
  MutexGuard 作用域 lint 均保留原日志。已按实际 API/明确作用域修正，未压制 lint。

最终源真实资格、独立只读证明及当前源检查追加如下。隔离脚本使用真实 Synapse/
Server/PG 和明确 whoami 检查的 OAuth fixture bridge，不声称生产 OAuth 或人工
桌面验收通过。完整计划见 owner-agent-capabilities-plan-2026-10-26.zh.md。

## 最终源真实资格与检查

最终 `owner3` exit0：新隔离目录
`/private/tmp/hagency-agent-crypto-owner-20261027-3/owner3`、新数据库
`agent_files_joint_owner_trust3`；日志 `/tmp/hagency-owner-verification-qualification3.log`。

- 两个真实 Agent 独立生成原始 SDK 主密钥/设备密钥。改变精确 master key 在签名前
  拒绝；主人 root 尚未验证时拒绝，完成实际 cross-signing 后才可验证 Agent。
- 另一个新登录的主人设备完成真实 SDK sync、公钥上传和主人 self-signing 签名，
  其 SDK 当前设备确实已交叉签名，且确认没有私有 user-signing key。它不能验证
  未受信任的第二个 Agent，返回明确恢复密钥诊断；第一 Agent 的验证不扩散到第二个。
- 正常主人 SDK 验证第一 Agent 后，关闭/drop/reopen 同一 NativeOwner 安装，
  设备授权1→2，原 Agent keys 与实际 Matrix 信任保留。主人 SDK 仍是同一登录，
  结果依赖重新读取的实际签名，不声称整个桌面进程/主人 SDK 已重启。
- 第二个 Agent 的原验证完成真实签名/回读后由资格 callback 暂挂；取消停止/
  shutdown 等待都不能提前确认，释放原 callback 后 checked shutdown 完成。
  callback 暂挂不是 HTTP body 暂挂；HTTP/body/并发队列由独立 SDK 集成测试覆盖。
- 独立 HTTP 从真实 homeserver 获取两个 Agent 的 master/device 公钥与主人
  user-signing 公钥；OpenSSL3 Ed25519 验证两条实际 master 签名均通过。各 Agent
  恰好一个 Matrix 设备，PG 固定摘要两条完整；原生账本 calls0、Server leases0。
  公共证明保存于 `owner3/independent-proof.json` 与 `signature-proof/`，不含私钥。

核心全套277通过/9既有忽略，桌面628通过/2既有忽略；最终6项核心及3项桌面
针对检查、受影响 all-target strict Clippy（App同CI使用 --no-deps）、core format、
i18n2542条/1916处调用/缺失0、46静态自测、1 production caller、1197 Rust bindings
均通过，无 missing/deferred/platformDeferred。最终 transport 不可用时隐藏缓存肯定
状态另有针对断言。SDK当前1+9及strict library Clippy通过，普通CI不运行外部资格。

三套本轮 Synapse、所有所属 Server 及专用 PG 已停止；原成功/失败目录保留。
上一头956f8980 fast/App/spelling通过；Full40457076793 Linux/macOS作业成功，
Windows实际格式/Clippy/全部测试/媒体重开/release步骤成功，缓存收尾仍待最终作业
状态。它们不能代替本批新源 CI，也不改写更早 Windows shutdown OutcomeUnknown。

## 下一段完整执行链

继续在现有租约/原始请求契约上接入 Agent SDK 的加密事件 intake、可信发送设备
和原事件证据、ACK、Codex 输入及原始加密回复/文件。Agent 对主人/请求者的信任
与当前 Server Room/成员/绑定/租约事实必须在真正执行/发送边界核验；不能使用
本 UI 的 ownerVerified 或公钥 DTO 作 grant。随后完成 A3、人工桌面、生产 OAuth
和既有跨平台门槛。完整 A4/总目标保持进行中。
