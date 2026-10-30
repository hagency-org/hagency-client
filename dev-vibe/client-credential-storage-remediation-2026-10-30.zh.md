# R05：桌面凭据存储迁移记录

日期：2026-10-30。状态：本地源代码已冻结，定向验证与独立源码复核通过；尚未提交本批 PR。专门的只读 SQLite 身份检查已接入已合并 SDK 的受审祖先 `95de9a4e`；最终整合到真实 main 后的完整套件、生命周期及当前 head 多平台 CI 仍待完成。本文不替代主质量报告的交付清单。

## 修复范围

- 桌面端明确选择 macOS Keychain、Windows Credential Manager、Linux Secret Service。默认后端缺失、锁定、拒绝或不支持时返回稳定错误，不切换 Mock 或明文。
- 操作系统存储只接收原数据库/Agent 小型秘密及包装密钥；大型 OAuth、会话和撤销 JSON 放在私有文件的认证加密信封中。上下文绑定 installation、purpose、完整身份及原引用。
- 数据库根密钥独立于 OAuth 客户端和授权代次。旧格式迁移保留原口令，系统写入与读回验证后才准备加密替换，不生成替代口令来打开既有非空库。
- 每次新浏览器授权前，先预留有限 SDK owner 槽并执行独立、非授权用途的系统写入/读回预检。已知不可用状态不会先发出注册或令牌请求。
- 原 SDK 的同步保存回调先保管实际最新令牌快照，再检查已解锁密钥和文件状态。容量在 SDK 激活前预留；同一授权的最新结果可替换自己的槽，不在回调时争取新容量。
- 原授权持有独立进程 lease。不同账号/授权不受全资料根的长期单实例限制；同一授权的原 SDK/真实 IO 未结束时不能并发恢复消费刷新令牌。
- 切换账号后的迟到结果保存在原授权 Keep 作用域；显式退出先耐久保存 revoke-only 意图。远端确认与本地退出区分，迟到的新 pair 继续进入原撤销记录，绝不恢复认证权限。后台在同一受跟踪工作线程每个空闲维护周期至多处理一项，交替优先 RAM 与磁盘来源，避免一个持续失败的 RAM 记录阻挡健康磁盘记录；不保证远端最终收敛或固定完成时限。
- 加密文件使用原文件 ID、摘要、私有 journal 和比较后替换。journal 保留自身物理 ID 与原句柄 witness；candidate 也由原句柄保管。rename 后的清理失败报告已发表但耐久结果未知，不回滚明文。
- 取消观察者不会结束已开始的系统调用。准备结果不再需要时，有原 RAM 密钥的真实结果会进入仅用于撤销的耐久记录；未知文件或阶段保留并阻止自动覆盖。

## 重要行为边界

SDK 保存回调的契约仍同步执行本地加密和文件 IO。远端轮换与本地提交不能成为一个原子事务；进程若在远端已经轮换、本地尚未确认之间崩溃，可能需要显式恢复或重新认证，不能自动重放未知刷新。

保留数据库的 LegacyDatabase 引用只证明原数据库材料，不构成 owner/device 登录授权。复用之前必须用实际 SQLite 账户身份核对真实 SDK 的 owner/device，保持原引用和口令。本批发现普通 `SqliteCryptoStore::open_with_config` 会初始化、迁移和 checkpoint，不能把它当只读身份检查。现在使用专门的同步 `inspect_existing_account_identity`：实际 SQLite READONLY/no-create、同连接读事务、拒绝未知/旧 schema，不调用正常存储初始化、迁移或 checkpoint；允许 SQLite 必要的 SHM 协调并读取已提交 WAL。原工作线程持有完整秘密、文件身份及任务收据，观察者取消不释放实际读取的所有权。SDK 以路径打开 SQLite 的既有平台语义仍适用；保留句柄及打开前后身份复核不等于防御同一 OS 用户下所有恶意路径替换。

没有执行系统凭据枚举或批量删除。旧授权的系统 key 可能继续保留；只有完整引用盘点及原所有者结束的证据，才可证明其未被使用。缺 key、未知 schema、未知 journal 或不完整目录遍历都不能授权数据库孤儿清理。

iOS/Android 使用精确的编译期 v1 私有文件兼容路径，R05 移动端保护仍未修复。读写都拒绝把桌面 v2/未知版本降级为 v1。主机上的兼容 helper 测试不等于移动设备或平台密钥库资格验证。

## 已执行的隔离验证

以下是旧 base 本地批次的真实结果；主任务将先整合已合并 main，再使用独立 Data target 执行完整验证，避免把跨树共享 target 视为最终构建隔离证据。

所有下列最终轮次显式设置 `HAGENCY_NATIVE_CREDENTIAL_IO=deny`，使用显式 Fake 存储和真实隔离私有文件。构建使用 Data 卷共享 target，没有读取或写入真实账户的系统凭据，也没有模型调用、真实分享或生产账户操作。

| 验证 | 实际结果 | 原始记录 |
| --- | --- | --- |
| core 第 9 轮凭据测试（SDK `95de9a4e`） | 17 通过，0 失败 | `.run/quality-credentials/core-tests-ninth-deny.log` |
| App 第 12 轮完整 lib no-run | 编译成功；仅既有 linker unwind warning | `.run/quality-credentials/app-build-owner-twelfth.log` |
| 第 12 轮默认并行凭据 focused | 32 通过，0 失败，0 忽略 | `.run/quality-credentials/app-credentials-fifth-default.log` |
| 原 OAuth 协议兼容 exact selector | 1 通过，保留刷新轮换、瞬时/终态错误及最新持久令牌断言 | `.run/quality-credentials/oauth-compatibility-eleventh.log` |
| App own strict 第 4 轮（最终代码） | `--no-deps --lib --tests -- -D warnings` 通过 | `.run/quality-credentials/app-strict-fourth-deny.log` |
| core strict（SDK `95de9a4e`） | `--lib --tests -- -D warnings` 通过 | `.run/quality-credentials/core-strict-third-deny.log` |
| Engine 较早完整 suite | 299 通过、9 个原有忽略；不是最新 mobile helper 后的最终 full | `.run/quality-credentials/engine-full-final.log` |
| Engine mobile/OwnerHost 最新 focused | 13 通过；不是移动设备资格 | `.run/quality-credentials/engine-mobile-final-credential-tests.log` |

17 项 core 覆盖写后读回失败、锁定/缺失、原文件替换、跨崩溃物理 ID、候选重试、已发表 unknown、原工作线程与取消队列、readiness 独立作用域和真实 journal/candidate 对象检查。

32 项最新 App focused 包含真实 loopback OAuth held refresh → 本地耐久 logout → epoch 退休及新账号 → 迟到 pair 仅进入旧撤销记录 → 两 token 实际 HTTP 撤销 → 冷恢复精确 SessionRetired；还包含 128 个实际 SDK owner 槽占满后原槽保存新结果、未准备好密钥的回调、取消 Native 准备的原 IO 收据、撤销部分成功/超时/重定向、数据库原 key/ref/历史连续性及 v1 拒降级。

复核发现的 pending 移除与并发 capture 竞态已使用 latest→pending 的统一锁顺序修复，真实 SDK 回调屏障回归在默认并行 focused 通过。后续独立复核又确认旧 SDK 读取与 latest 替换之间存在乱序窗口，现将实际读取、单调 revision 和替换放在同一短锁内，离开后才进入 pending/SESSION_IO；新增真实 HTTP 刷新与原 IO 屏障回归已在第 12 轮二进制通过。原 SDK handoff 的真实 HTTP 被持有期间，取消观察者仍保留原 Client、数据库材料和 lease，真实操作结束后才释放；旧 epoch 不发表认证。共享 AuthCtx 的真实 notification client 在原 ClientInner 消失后仍阻止同 grant 恢复，最后实际 SDK 回调所有者释放后才允许恢复。复核还修复了已正常发表且 journal 已删除的取消准备阶段：没有实际 SDK 快照时，原 canonical、candidate、Keep、Revoke 都必须绑定同一原 grant 并通过 AEAD 认证，按认证 revision 选择最新；其他 grant 不参与，也不会被删除或重写。新增真实刷新、实际 SDK drop、旧 Keep B 与新 canonical C 的回归已验证持久 Revoke C，不代表该单项已实际完成远端撤销。维护来源交替的回归通过同一原工作线程两轮真实请求，验证 RAM 503 后下一轮磁盘 200 的认证 receipt 已耐久保存，RAM 原 pair 与有限槽仍保留。以上不替代整合后完整 suite。

第 10 轮默认并行 focused 的 27 通过/1 失败原记录保留。失败原因是测试隔离根受 umask 为 0755，而私有目录检查要求所有者独占；同一原二进制的受控 0755/0700 对照确认此因。只将 fixture 改为生产私有创建接口创建全新资料目录，没有 chmod 既有目录或弱化权限检查。

App 首轮 strict 命令遗漏 `--no-deps`，触发未改动 `article-makepad` 的 27 个现有 lint 并失败；保留 `app-strict-first-deny.log`，不把它当通过或修改这些依赖来掩盖。后续使用与 CI 相同的 own-crate `--no-deps` 边界，最终 lib/tests strict 已通过（未替代最终 CI 全 feature/all-targets 门禁）。第二轮发现的 5 个本批 lint 已机械修正，没有添加豁免。

先前一次未带 deny 的 lifecycle 尝试在 Cargo 阶段被终止，没有测试输出，不计入通过或系统凭据资格证明。最终生命周期、整合后完整 App/Engine suite、整合后 strict、三平台 CI、许可证及新依赖审计均待源冻结后统一执行；既有定向 strict 结果仅代表其记录的源和目标。

## 尚未获得的资格

- 未操作真实 Keychain、Credential Manager、Secret Service 命名空间，未验证真实弹窗、解锁/拒绝、崩溃恢复或安装包用户流程。
- 未验证生产 OAuth、移动设备或跨主机凭据恢复。
- 不声称 SDK 同步回调已变成可等待的异步耐久协议，不声称所有磁盘/权限故障都能自动恢复，不声称旧系统 key 已全部清理。

## R19：自动数据库回收暂停

完整引用快照不构成跨进程互斥。复核发现，盘点完成后另一个应用实例仍可新建
数据库与授权，旧清理器不能把快照中尚未出现的新目录当成孤儿删除。本批暂停
生产启动时自动数据库目录删除，保留诊断/测试，不以 mtime 抽样或未被所有创建者
遵守的伪锁代替协议。旧数据库及旧系统 key 可能继续占用空间；自动空间回收仍待
统一跨进程创建/回收协议交付。多进程实际生产入口保护回归已在第 12 轮 fresh 二进制通过；其真实负对照验证旧清理算法会用旧快照删除另一进程新建的数据库及缓存。

Matrix 访问令牌已失效而刷新令牌仍未确认撤销时，当前策略不会为了退出而重新
刷新令牌；远端状态可能长期 pending。新的迟到 pair 可推进原记录，但重试不
代表所有远端会话最终一定被终止。

原生 Agent/独立 server-login 的库路径在 Linux 也需要可用 Secret Service 与用户
D-Bus；缺服务的 headless/container 环境明确失败，不会隐式选择文件或 Mock。
真实 Linux 服务/容器凭据行为未获资格验证，旧产品 CLI/无界面安装器仍未恢复。


## 整合后完整并行测试的隔离修正

整合 main 后的精确 CI profile 首轮实际为 851 通过、5 失败、2 个原有忽略，
不能计作全库通过（主树原记录 `app-ci-profile-full.log`）。5 项失败均进入了
凭据加载前的全局 pending 保存；最初的 epoch 并发猜测不是已证根因。

已用未修改的 `60058011` 二进制做真实因果对照：一个进程串行运行原 OAuth
roundtrip 后再运行 `session_preserves`，实际 1 通过/1 失败，后者精确重现
`credential_storage_failed`（`.run/quality-credentials/shared-pending-two-case-negative.log`）；
新进程单独运行同一完整名称 case 实际 1 通过
（`.run/quality-credentials/shared-pending-same-case-alone.log`）。第一次简称加
`--exact` 的诊断只选中 0 项，原记录保留，不计入通过证据。

原 OAuth 测试故意删除会话文件后再真实刷新，以断言已移除会话不会被重建；
保存回调按设计保留脏快照，测试随后删除其资料目录。该进程全局 pending owner
在回调家族释放后，仍会被其他测试的加载入口尝试保存到已删除目录。
这证明了测试 App 生命周期互相污染，而不是应当弱化生产围栏的理由。

仅将这个 producer 及 5 个 consumer 放进现有独立子进程 helper：每个使用全新
私有资料目录、显式 Fake/Native deny、精确单项 selector 与 60 秒 watchdog。
故意删会话→真实刷新、原数据库/历史/令牌断言全部保留；未清空生产 pending，
未改生产 epoch/flush/授权逻辑，未降低完整 suite 的默认并行度。该 test-only
补丁的 fresh 因果两项与完整默认并行重验由主任务执行，不能用旧二进制代替。


主任务另核对首轮 full 原始 stderr，发现远端审批真实 SDK fixture 的后台线程虽未令 harness 失败，却因通用 `capture_put_to_device_traffic` 把合法 `m.secret.request` 的 `*` 当成显式 device 而 panic，并导致队列锁 poisoned。不能把该用例的表面 `ok` 当成完整通过。现仅在项目测试中精确捕获 `m.room.encrypted` 并保留原 SDK queue/device 断言；另外只应答当前 owner、结构和 action 合法的秘密请求，未放宽跨签名、只向可信 REMOTE 分享、拒绝 UNSIGNED 以及原 journal 断言。新增 matcher 拒绝 foreign/unknown/malformed/empty；不修改 SDK 或产品路径。修改后的真实审批 case、完整并行套件仍待 fresh 执行。

整合树新鲜主锁审计仍为 11 条匹配且 exit1，不能标记供应链风险已关闭。当前 cargo metadata 共 1336 个 package；新增 keyring/Secret Service/security-framework 依赖的许可元数据存在，另有 7 个既有包缺少 Cargo license/license_file 元数据（不等于已证明无许可）。完整平台与发行物许可证核验仍开放。Windows platform crate 的 all-targets 交叉检查通过；store 交叉检查因本机缺少 x86_64-w64-mingw32-gcc 未完成，不能代替原生 Windows CI。静态布局、当前 native caller 审查、50 项契约检查及发行资格脚本通过；系统默认 Python 缺 tomllib 的原失败日志保留，改用已安装 Python3.12 后复验通过。引擎格式门禁发现新模块导出顺序后已机械修正，不改变行为。


## Engine all-targets 集成 fixture 闭合

主树 fresh `cargo test --locked -p hagency --all-targets` 完整 exit101，38 个 targets
共 586 通过/4 失败/62 个原有忽略，qualification.rs 另有 1 项 operator filter。
其中 lib 为 302 通过/9 忽略，console 集成目标为 126 通过/4 失败/6 忽略，
不能只用 lib 结果描述为全 targets 通过。
原记录 `.run/quality-credentials-integration/engine-final-full.log` 保留。三项在本地
退出返回 503，一项在账号切换返回 400；四者共同的集成 Fixture 仍调用 Native
默认构造，耐久撤销保存因自动化 `HAGENCY_NATIVE_CREDENTIAL_IO=deny` 按设计拒绝。
这不是系统服务或原生产保存围栏应当宽免的依据。

仅在集成测试目录显式构造独立 Fake SecretStore/Vault，Fixture 全部经已公开的
显式凭据构造器；同进程重启和原生 facade 重新打开复用原 Vault。原先直接解析
明文撤销数组的断言改为读取实际 v2 envelope、验证完整原安装域/Revocations
scope 并用生产 Vault/AEAD 解密后执行相同数组、hint 和清空断言，同时断言落盘
schema2 且不含原 token。未删除 deny、跳过用例、引入生产 testenv 后端选择或
文件 fallback，未修改刷新/远端 HTTP/本地失效/DB 断言。fresh focused/all-targets
验证仍由主任务执行，当前修改本身不计作测试通过。

候选空间读取的单独 OwnerHost fixture 保留原真实生产路由；其惰性 Native 构造
不执行凭据 I/O，case 不走本地撤销持久化，自动化 deny 仍在。没有为此公开额外
生产 API，也没有将该候选读取通过描述为真实 Native 存储资格。


第二轮 Engine all-targets 的 console 目标为 128 通过/2 失败/6 原有忽略，
原记录 `engine-corrected-all-target-full.log` 保留。两项失败出在新测试读取
helper：旧 `private::read_secret` 明确只接受最多 512 字节的小秘密，正常 v2
scope/envelope 已超过此上限，因此返回 `Private`；不是已证的权限或硬链接
异常。测试观察若直接获取 Document 的 try-lock，会与 offline 后台 writer
竞争；因此仅将 helper 改为现有 `private::open(path, false)`，保留原私有权限、
no-follow 和句柄检查，再以 `Read::take(1MiB+1)` 实际累计读并断言不超过 1MiB。
原 FD 在 atomic rename 后仍指向其原始对象；下一轮再 open 才观察新版本。
这些观察字节不授权 publish、repair 或恢复认证。Engine 生产撤销正文上限是
8MiB，这里的 1MiB 是更小的测试观察界限；没有放宽生产 cap、权限或链接校验。
完整 v2/scope/AEAD/no-token 断言保留。此修正仍待 fresh 验证，不将前轮
2 项失败或继续运行的旧二进制结果回填为通过。
