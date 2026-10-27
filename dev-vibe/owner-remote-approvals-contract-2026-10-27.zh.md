# 当前主人远程审批实施与验收

A4必须同时完成私密远程审批及当前Agent加密执行。本批已接入执行设备的私密发布、
原活跃PendingApproval消费、原生管理命令和中英文桌面入口。另一台设备的审批
收件箱、完整操作展示、一次性加密决定发送及原投递查询也已接通。真实Synapse
原生联合验收及Agent加密身份/任务仍待完成。
Server的加密Room执行拒绝条件继续保留；A4和总体目标保持进行中。

## 完整接入顺序

| 阶段 | 当前状态与交付 |
| --- | --- |
| 原提案与单次发布 | 已实现。绑定主人、部署profile、审批ID、原执行、绑定代次、分发epoch、执行设备、发布SDK设备、私密Room及完整提案摘要。首次提交产生Publication，只能取出一次SendPermit。 |
| SDK读取与验证设备 | 已接通实际SDK解密、当前主人身份、交叉签名设备、公钥、原事件回复关系及摘要。本地HTTP替身上的真实双设备密码学测试通过；真实Synapse/原生联合验收仍待完成。 |
| 私密发布与当前运行 | 已接通。原活跃审批槽持有同一个账本和Publication；SDK使用新的单事件Megolm密钥，仅发给主人当前已交叉签名设备；未知不重发，原IO留在运行时收尾任务中。 |
| 原生远程审批操作 | 执行设备按Room名称选择候选、私密发布、查看原状态和自动读取经验证决定。另一台设备在Agent的“远程审批”页查看实际SDK验证的原请求、完整操作、批准一次或拒绝、复制完整原提案、查询原决定投递；无需本机运行provider或取得Agent执行分配。 |
| Agent加密身份 | 待接入。为每个当前Agent及执行设备建立自己的SDK加密身份、设备生命周期及私密存储；不能把Server AS凭据发给客户端，也不能借主人密钥伪装Agent。 |
| 加密任务与主人私聊 | 待接入。加密输入在客户端验证/解密，模型输出及文件经原Agent SDK加密；Server保留原意图/设备租约和密文投递见证，不能收到解密正文或文件密钥。覆盖撤权、未知、恢复及远程裁决。 |

A2文件工具、A5报表告警、A6恢复审计、A7备份与A3协作和各平台/人工验收仍沿用
总计划；本批未声称A4或总体目标完成。

## 本批持久化边界

remote_approval模块使用现有approval_audit和原local_inbox。公开Request/Verdict
是消息数据，不能从反序列化消息得到Publication或ObservedVerdict。后者只允许
可信宿主在真实SDK核验后构造；库自身不验证Matrix签名，调用者不能把JSON中的
verified、sender_device或签名字符串当成事实。

首次BEGIN IMMEDIATE提交固定原请求，返回不可Clone/序列化的Publication。它还
绑定原Ledger句柄的进程内身份；另一个句柄、重开同一文件、快照恢复或复制元数据
都不能使用它写入裁决。已有见证不能重复预留，改Room、设备或参数也不能替换。
Publication保留给原回执与裁决使用；实际SDK发送必须消费单独的SendPermit。
同一Publication只能取出一次凭证，丢弃、取消或原尝试失败后均不能重新取得。
保留最多4096条、实际UTF-8请求合计8MiB、单请求32KiB，超额不插入发布行，也不
删除旧见证腾出重发权限。

原SDK回执仅关联实际原事件ID，冲突回执拒绝；晚到回执可以保留历史，但不能恢复
审批或发送权限。裁决绑定原回执、不同于发布设备的已验证主人设备、公钥、完整
请求摘要、执行设备代次、原运行中执行、分发epoch与三层策略版本。主机读取证据
最多使用5秒，从首次SDK检查开始计时；网络或SQL等待不会刷新旧证据的年龄。

裁决行和原audit的OwnerAllowed/OwnerDenied在同一个事务提交。取得实际SQLite
写锁后重新采样宿主时钟、读取执行、策略、期限及原审批阶段，避免写锁等待期间的
变化；倒退到发布或回执之前的时间也拒绝。返回的
RecordedDecision只是决定证据，不写approvals工具权限表，也不代表工具已执行。
运行时重新检查实际服务器绑定、执行租约/设备和原活跃审批槽后，才将决定交给
原PendingApproval一次；正常工具路径仍检查当前策略和执行权限。发布与裁决API
接收宿主当前时钟函数，不能用旧HTTP参数或消息时间戳充当当前时间。

发布任务在调用方再次yield前加入原运行时joins。审批移除、turn取消、Agent停止、
设备失效、原期限到达或provider决定接收端关闭，均撤销后续写入权限；只读弱引用
不能延长或重建原决定发送端。已经发生的原SDK接受仍可记为晚到历史，不能恢复
运行或重发。读取状态可使用历史元数据，但消费远程决定必须持有原进程内句柄。

## SDK读取边界

App共用原SDK客户端及当前账户存活检查。最多4项读取，整体30秒，HTTP单次8秒，
状态/事件响应最多128KiB；固定当前homeserver、认证token和生成的Room/event路径，
拒绝重定向结果。不读取provider登录文件或刷新凭据。

新鲜Room状态必须为普通加密Room、invite、禁止guest、joined或invited历史可见性，
主人joined且没有其他joined/invited/knocking成员，重复状态键拒绝。实际事件必须
为m.room.encrypted，经Room.decrypt_event解密；只有SDK VerificationState::Verified
可继续。明文、无法解密、未验证、转发来源、发布设备自回声、编辑、redaction、
错误Room/原请求/摘要均不能成为裁决。解密前后刷新主人身份及当前设备公钥，要求
身份已验证、设备已交叉签名且未删除，并与加密会话声明公钥逐字相同。每次异步
操作后重新核验当前账户；最终设备读取后也检查存活和Room joined状态。

自动查找仅查询原发布事件的m.reference/m.room.encrypted关系，最多20条；出现
分页、重复、错误关联或无法验证的候选时不授予权限。每个批准候选均经实际SDK
验证；最终选中事件重新读取验证以取得新证据，不能刷新旧证明的时间。已验证拒绝
只撤销原权限。没有候选显示等待，用户无需输入event ID。关系查询依据
[Matrix Client-Server规范](https://spec.matrix.org/v1.19/client-server-api/)；
内层解密内容也必须关联相同原事件，服务器给出的关系不能独自证明授权。

原有envelope守卫使用明确的SDK元数据夹具，不能单独证明身份。新增crypto_tests
使用实际SDK公开交叉签名/设备验证、Olm/Megolm、加密发布和另一台签名设备决定，
最终写入实际原账本。另一个真实未签名设备伪造verified和设备ID仍被拒绝；新加入
外人成员的Room在消息写入前被拒绝。这些测试的HTTP homeserver是本地替身，
不是Synapse或人工桌面验收；NativeOwner运行时测试也明确使用Matrix来源替身。

## 发送必须加密的SDK边界

现有SDK的send_raw根据房间缓存决定是否加密，默认还会明文发送reaction。
因此，先检查缓存、再调用普通send_raw，不能作为私密审批的最终发送保证。
当前固定的0.19.1 fork增加require_encryption()；不支持加密或房间未加密时，
在消息HTTP写入前返回EncryptionRequired，显式要求加密的reaction也执行加密。
这个选项不验证收件人身份或私密成员范围。当前私密审批另用独立的一次性发送路径。

Client的8项SDK补丁已固定到1c48a831ded0dc7fddcb74d2d4e3181398b6aee2；
只更新同一0.19.1 fork的发送边界，保留原bounded history补丁和其他依赖版本。
已有加密续接消息也使用该入口。SDK修复见
[SDK PR1](https://github.com/chrislearn/matrix-rust-sdk/pull/1)，基于Client原先固定的
chris/bounded-chat-history；没有将Client升级到SDK较新的main。

seal_for_own_devices创建独立单事件Megolm会话，不读取或写入可复用的Room发送
会话缓存；即使原普通Room密钥曾发给未签名设备或外人，新审批密钥也不会复用。
当前主人身份和本设备必须已验证/交叉签名，远端已签名设备数量限定1..64。仅
保留本机入站会话及已推进的Olm状态，返回不可复制/序列化的密文发送所有权。
消费时每次to-device/Room写入前检查原权限，关闭SDK写重试；未知结果不返回所有权。
App在封装前后重新检查实际私密状态，发布最多4项/60秒；读取和搜索各4项/30秒。

SDK新增2项实际密码学HTTP测试验证原宽泛密钥不污染审批、签名远端能解密、
未签名/外人不能解密、普通后续消息仍用原普通会话，以及撤权/丢失接受响应不重发。
当前history特性下SDK和crypto library严格Clippy及原必须加密3项回归通过。
App新增SDK testing/wiremock仅为dev-dependencies；lock新增相应测试依赖。

实际SDK本地HTTP测试验证拒绝时消息写入为零、消息和reaction输出Megolm密文，
以及普通明文兼容：启用加密时3项通过，不启用加密时2项通过；Client使用的
bounded history组合也通过3项及严格library Clippy。未启用该history特性的严格
Clippy仍在原缓存代码报question_mark；原失败保留，没有通过allow压制。
首轮新增测试因SDK要求test_函数名前缀而未编译，修正名称后的结果单独保留。
SendPermit回归及Agent-local全量为156通过、3个原忽略。日志前缀为
/tmp/hagency-required-encryption和/tmp/hagency-remote-send-permit。

## 前批回归与兼容

本机Agent-local155通过、3个原忽略保留；运行引擎263通过、7个原忽略与2个外部资格
忽略分开；App622通过、2个原忽略保留。新增13项核心回归及2项SDK守卫覆盖单次
预留、真实磁盘重开、原事务裁决、反向/错误绑定、策略变更、过期、实际容量、
停止、原回执、永久否决和快照恢复。当前core/App默认功能all-targets严格Clippy、
core格式/拼写、46项守卫、一个生产caller和1176个契约selector通过，missing、
deferred、platformDeferred均为空。日志以/tmp/hagency-remote-approval为前缀，
agent-proof-final与clippy-proof-final为最终源检查，bindings-clock-final为绑定检查。
原有全App格式漂移未批量改写。

专门的SQLite写锁竞争用例先失败，证明锁前检查会接受旧策略；改为锁后事务内
重检后通过。旧快照夹具最初保留了新模块空表而成为从未发布过的混合schema；已
在仅测试的旧格式构造中删除新增空元数据，保留生产精确schema白名单和旧数据
断言。真正的前一schema快照保留原audit，迁移仅新增空见证。

初次编译中误用policy/restore方法、测试断言要求敏感Request的Debug、错误SQL列和
SDK枚举/参数签名的失败都保留在本轮日志。未使用allow压制、重写失败结论或扩大
产品期限。真实Synapse私密发布/双设备裁决、人工Native/UI和加密Agent运行仍必须
另行验收。

## 当前批验证记录

当前源Agent-local157通过/3个原忽略，运行引擎269通过/9个忽略（7个原忽略与2个
外部资格门槛），App624通过/2个原忽略。包含6项实际NativeOwner/TCP/SQLite/
原活跃PendingApproval测试、弱存活引用回归、实际SDK密码学及桌面组件回归。
日志为/tmp/hagency-remote-core-full2.log和/tmp/hagency-remote-app-full1.log。
当前core/App默认功能all-targets严格Clippy（App使用正式CI的--no-deps边界）、
core格式、拼写、46项守卫、一个生产caller和1184个selector绑定通过；missing、
deferred、platformDeferred均为空。中英文检查2507个条目、1893个调用点，缺失0。
日志为/tmp/hagency-remote-current-clippy3.log、current-bindings1.log、remote-static1.log。
严格检查先指出新增remote_details位于测试模块之后，已移动到模块之前；原失败
current-clippy2.log保留，没有添加allow。实际SDK/App密码学测试日志为
/tmp/hagency-own-device-sdk-test3.log和/tmp/hagency-remote-app-crypto-related.log。
初次SDK测试的Raw类型错误、UI变量遮蔽、生产PendingApproval构造未同步以及
修正构造时未限定Arc/Mutex作用域的编译失败均保留，不计为通过。

App密码学夹具最初默认token匹配错误；随后使用不同priority的ScopedMock又暴露
wiremock0.6.5排序后按旧索引停用错误mock的问题。已改为一个固定注册的动态状态/
事件响应器，不修改依赖缓存或生产守卫。test3至test6保留失败，test7和related
分别记录改正夹具后及自动关联查询后的成功；没有把原失败改成成功。

最初把App与核心一起执行不带--no-deps的严格Clippy，触发未修改的article-makepad
27项既有lint。App正式CI使用--no-deps；按该边界另跑当前包检查，不修改这些依赖
代码或添加allow。原失败日志/tmp/hagency-remote-current-clippy1.log保留。

先前8c29ed8e完整远端矩阵40426784089已结束，Linux/macOS/Windows实际job均成功。
它不能代替当前源验收。更早A2的0f7a5a7c完整矩阵40423919137仍保留Windows
fixture.store.shutdown返回OutcomeUnknown的失败；后一次通过不说明其根因已解决。

## 远端审批收件箱与决定发送

原生RemoteApprovals命令只在NativeOwner进程内执行，当前SDK账户、主人、origin/
issuer/subject和profile必须一致。历史列表每页最多10个加密事件，响应最多128KiB，
固定认证homeserver/Room路径；游标最长1024字节，拒绝直接重复。界面最多128条并
阻止已使用的游标循环。普通已验证消息/本设备原请求不成为待批项；其他设备请求
必须经SDK真实解密、Verified来源、当前交叉签名设备及实际Ed25519会话公钥核验。
正文中的publisher设备声明必须与真实加密来源一致。过期请求不可决定。

列表内容只是展示。点击批准/拒绝时重读原事件，检查用户刚审阅的完整request摘要、
当前主人/profile/Room/Agent和不同的Matrix设备。ObservedRequest不支持Deserialize；
不能用UI JSON、verified字段或旧列表证明重新发送。实际SQLite写锁之后再采样
当前宿主时间，证明年龄最多5秒，原提案期限不延长。

远端决定使用同一主人账本中的remote_verdict_publications和不可改写原回执表；
没有本地原执行或approval_audit也可记录这个远程发送见证，但不能创建工具授权。
首个事务提交就永久记录原approval ID、原事件、请求、公钥、审阅设备和批准/拒绝。
返回的发送所有权不可复制/序列化，只能取出一次。丢弃、冲突选择、不同事件、磁盘
重开及快照恢复都不能重建。最多4096条、实际序列化UTF-8合计8MiB，不删除旧记录
腾出重发机会；旧版精确schema快照仅迁移空表，现有格式保留全部原决定/未知见证。

SDK发送前再次实际核验原请求和当前私密Room，再用独立单事件密钥加密决定。
每次写入重检原账户/设备存活和原期限，关闭SDK重试；最多4项原发送，60秒超时。
任务在返回界面前进入NativeOwner收尾集合，并持有更新屏障，更新不能穿过未结IO。
关闭先撤销后续写权限，再等待原任务；实际晚到接受可记录历史，未知不重发。
UI在决定未确认时禁用批准/拒绝，只能读取原状态；“已投递”仍不等于实际执行完成。

当前验证先后保留原失败：核心测试复用夹具最初未开放模块内可见性；SDK子模块
入口可见性不足；Native测试误用了等待方法名及私有字段。修正测试/模块边界后的
独立结果不改变原失败。未改依赖缓存或放宽生产验证。

本轮实际SDK密码学测试已从手写远端密文扩展为调用真实收件箱、原请求检查、持久
单次决定和SDK发布，再由原执行设备验证并提交原账本。仍然是本地HTTP homeserver
替身上的真实密码学，不是Synapse、人工UI或实际provider消费整条链的验收。新增
NativeOwner测试使用明确的Matrix来源替身；组件测试实际创建Makepad控件，验证
另一台设备可审阅超过20000字符的完整命令、未知不可再批及scope清理。

本轮Agent-local162通过/3个原忽略，运行引擎271通过/9个忽略（7个原忽略与2个
外部资格门槛），App625通过/2个原忽略。更新屏障补齐后，2项Native远端决定测试
另跑通过。当前core/App默认功能all-targets严格Clippy（App使用正式CI的--no-deps
边界）、core格式/拼写、46项守卫、一个生产caller及1191个selector通过；missing/
deferred/platformDeferred为空。i18n2520个条目、1906个调用点，缺失0。
日志前缀/tmp/hagency-remote-review；core-full1、app-full1、native-final、clippy2、
bindings1分别保留实际结果。严格Clippy最初指出新UI中同行的独立if疑似漏else，
已换行表达独立条件，clippy1原失败保留，未添加allow。
最后另查等待超时分类：收件箱/原状态是只读，决定发送不可按只读超时脱离原任务；
针对性回归与最终clippy-final日志单独保留。

上批ff496b83的CI/App/完整矩阵仍是独立运行，本轮未因观察时间过去而重启这些
任务。A4/A3与先前平台/人工/OAuth门槛继续保留，未宣布总体目标完成。
