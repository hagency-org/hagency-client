# Agent 独立身份：正常生命周期接入

本批将已实现的身份专用 broker/SDK 接入正常 NativeOwner 与桌面 Service 界面。
工作分支为 `codex/remote-owner-approvals`；修改前重新 fetch `origin/main`，
最新 `42e59b9e` 已是当前分支祖先。没有在 main 工作或回写另一份源码目录。

## 实现与边界

- 正常创建 Agent、指派到当前设备、恢复 Agent、成功启动本地 runtime/service 后，
  由原生宿主初始化独立 SDK 身份。创建/启动的原结果保持独立，身份失败附带明确
  `unavailable` 状态；不会把一次已创建的 Agent 伪装成未创建而重发创建。
- 暂停、退役、停止 service 时停止原会话；显式 Prepare/Status/Stop 命令仅在原生
  宿主执行。正常 Source 必须明确支持此能力，旧 Source 默认不启用。
- 每个主人 profile/Agent/稳定执行设备使用独立私有目录、64字符随机 passphrase
  和加密 SDK SQLite。凭据采用已有私有文件接口（Unix owner/mode/nofollow、
  Windows DACL），文件内容和临时序列化缓冲区清零；未导出到命令、UI、Server
  或账本备份。**这是受私有文件权限保护的本地凭据存储，未声称 OS keychain。**
- 授权/Agent generation 不更换原密钥目录；原 credential 缺失而 SDK 目录已存在、
  绑定不符、symlink 或原始 SDK key store 缺失时拒绝重建。换主机、恢复和重置仍
  未开放，不会生成替代身份绕过 Server 的永久公钥固定。
- NativeOwner 保留弱宿主授权和原初始化/关闭任务的共享回执。取消某个 UI/超时
  等待者不会删除原任务，后续停止与 checked shutdown 必须等待同一个原任务。
  停止先永久退役权限，不会在迟到初始化完成后发布 ready。
- SDK fork `368152a8` 增加可选请求 fence：每次实际 HTTP 尝试在并发槽等待后、
  每次重试前重新检查当前权限，并将原 permit 持有到实际响应 body 结束。
  retire 永久拒绝新请求；drain 等待原请求，不中止/伪造其历史结果。
  自定义原始 HTTP 同样持有 permit；SDK 初始化任务在发布/关闭时等待结束。
  SDK Client/原请求 fence 同时保留独占 store lease，避免宿主 drop 后过早重开。
- 原生刷新读取当前授权，不改变同代次会话或密钥；新授权代次要求重新打开原 store。
  初始化全局四个槽；每个宿主最多32个已打开或正在初始化的身份、128项状态。
- 桌面 Service 显示身份状态及完整公开 user/device/Ed25519/Curve25519，提供
  刷新、初始化、停止和复制公开身份。响应按主人/Agent/代次/执行设备隔离；切换
  主人、Agent 或记录变更会清除旧展示，其他执行设备禁用本机身份操作。
  文案明确“主人尚未验证”，不将公钥就绪说成加密任务可执行。

## 验证

- 新增核心5项测试覆盖凭据独立性、原目录/秘密保留、丢失/错绑/symlink 拒绝、
  取消停止与初始化等待后仍由原 checked shutdown 收尾。当前核心全套276通过、
  9项既有忽略；最终小修改后的5项全部通过。
- 桌面新增实际 Makepad 脚本构建测试，覆盖完整公开身份、未验证文案、错误 Agent/
  generation 忽略与其他执行设备按钮禁用。App全套628通过、2项既有忽略。
- SDK实际 HTTP 请求 fence：1项单元与4项集成通过，覆盖等待并发槽后失效、实际
  503后的退役禁止重试、响应 body 在途时 drain 等待和未配置 fence 的普通兼容行为。
  既有 require_encryption 三项、独立 own-device Megolm 两项回归及 strict Clippy 通过。
- 最终源真实资格使用 `agent_crypto_managed_qualification`，与历史手工 facade
  driver 分开；普通 CI 仅编译，必须显式隔离环境才可运行。不调用模型。

### 真实资格记录

隔离根目录 `/private/tmp/hagency-agent-crypto-managed-20261027-1`；全新数据库
`agent_files_joint_managed1`，实际 Synapse 1.163.2、现有 Server393fde9、独立
NativeOwner/SDK。OAuth 仍是实际 whoami 检查的显式 fixture bridge，不是生产 OAuth。

- `managed1` 失败：自动初始化误用了 `/console/owned-agents`，既有正确本地
  路由是 `/console/api/owned-agents`，创建结果附带身份 unavailable。原失败日志
  `/tmp/hagency-managed-crypto-qualification1.log` 保留。已改用现有 AgentDetails
  命令生成路由，不增加重复路由常量；旧 Agent 不重发或删除。
- `managed2` 成功：正常 CreateAgent 自动初始化两个真实 SDK，身份彼此及与主人
  独立；35秒后同一会话续期、公钥不变且 prepare 次数仍为2；独立 homeserver
  公钥核对；实际 API 撤销及直连 HS bearer401；同一安装重开并保留原公钥。
  第三个真实 SDK 已准备好、返回 NativeOwner 前由资格 callback 暂挂，取消停止/
  shutdown 等待均不能提前确认；释放原 callback 后 checked shutdown 完成。
  **callback 暂挂不是 HTTP body 暂挂**，后者由独立 SDK 实际 HTTP 测试验证。
- 后续最终源资格与独立只读证据见本节追加记录。

编译失败也保留：首次 UI 子模块方法未标记 `pub(super)`、资格驱动误将
`canonicalize()` 的 Result 直接比较；首次 strict Clippy 报嵌套 if 与复杂返回类型。
已修正实际方法可见性/错误传播，折叠 if 并定义明确的宿主 preparation 类型；未
压制 lint、放宽权限或将失败改写为通过。

## 仍未完成

主人对 Agent 的实际验证、跨主机/密钥恢复、加密任务原事件接收/解密/ACK、
当前成员/绑定/租约事实核验、原始加密回复和文件，以及 A3 多 Agent 协作。
现有 encrypted-Room 执行拒绝继续有效。既有人工桌面、平台和生产 OAuth 门槛
保留；未付费重跑 Codex、未部署或合并，总目标继续进行。

## 最终源验收与收尾

`managed3` 最终源再次通过，进程 exit0；原日志
`/tmp/hagency-managed-crypto-qualification3.log`，结果与独立只读证据分别在
`managed3/result.json`、`managed3/independent-proof.json`。两个主要 Agent 为
`agt_01m5xxv65crpj7zjjvxkvwnk9e`、`agt_01m5xxv67bmdzfja8018x1wvje`。
独立 Server 读取确认两者 first_device_generation=1、当前 generation=3、全部
四项固定摘要存在，实际 Synapse 各自恰好一个 Matrix 设备且两类公钥一致。
第三个被暂挂的真实 SDK 也保留完整固定摘要。原生 calls=0、Server leases=0。

当前最终 strict core/App all-target Clippy（App同 CI 使用 --no-deps）、核心格式、
桌面3项针对回归、中英文2534条/1913处调用/缺失0、46项静态自测、1个生产 caller、
1196项 Rust spec bindings 均通过；missing/deferred/platformDeferred 均空。
相应日志以 `/tmp/hagency-managed-crypto-` 为前缀。全套核心与桌面回归后，最终
资源锁保留修改另以针对回归与真实 managed3 验证，没有付费模型调用。

本轮拥有的 Server、Synapse容器 `hagency-files-synapse-f5245e6d6e` 和独立 PG
均已停止；NativeOwner checked shutdown 完成，全部原目录及失败记录保留。
前一提交db6f的 fast/App/spelling，以及 Full40450763616 实际 Linux/macOS/Windows
全部通过；该 CI 不是本轮新源的验证，新头将另行提交远端检查。


后续已接入主人对精确 Agent 身份的实际交叉签名/回读，原记录中 ownerVerified=false
保留为该轮真实结果。当前实现与 owner3 独立签名证明见
[精确主人验证验收](agent-crypto-owner-verification-2026-10-27.zh.md)。该信任不开放
加密任务/租约/消息权限；这些完整路径与 Agent 侧请求者信任继续实施。
