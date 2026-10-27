# 当前主人远程审批实施与验收

A4必须同时完成私密远程审批及当前Agent加密执行。本批交付原审批的持久化发布与
裁决见证、快照兼容及实际SDK裁决读取代码，尚未接入私密发布、运行中的
PendingApproval、原生审批操作或Agent加密身份。没有启用远程授权入口，也没有
放开Server的加密Room拒绝条件。本契约是A4第一阶段，不改变总体交付范围。

## 完整接入顺序

| 阶段 | 当前状态与交付 |
| --- | --- |
| 原提案与单次发布 | 已实现账本部分。绑定主人、部署profile、审批ID、原执行、绑定代次、分发epoch、执行设备、发布SDK设备、私密Room及完整提案摘要。首次提交产生Publication，只能取出一次SendPermit。 |
| SDK读取与验证设备 | 代码和守卫夹具通过。通过实际SDK解密，核验当前主人身份、交叉签名设备、公钥、原事件回复关系及摘要；真实双设备加密验收尚待完成。 |
| 私密发布与当前运行 | 待接入。由当前PendingApproval持有原账本和Publication，经过SDK当前账户与私密Room检查发送，未知不重发；结束时保留原IO收尾所有权。 |
| 原生远程审批操作 | 待接入。主人配置私密审批室、另一台已验证设备查看完整提案并批准一次或拒绝；直接使用当前主人协议，不借用旧Fleet绑定或宽泛可重用授权。 |
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
RecordedDecision只是决定证据，既不写approvals工具权限表，也不向模型发送批准，
更不代表工具已执行；后续必须消费原活跃PendingApproval并再过实际服务器/租约/
设备/策略检查。发布与裁决API接收宿主当前时钟函数，不能用旧HTTP参数或消息
时间戳充当当前时间。

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

这条读取路径已实际编译；目前新增SDK正向分支使用明确的DecryptedRoomEvent元数据
夹具，未伪称真实交叉签名或双设备现场验收。也没有新增网络审批请求发布。

## 发送必须加密的SDK边界

现有SDK的send_raw根据房间缓存决定是否加密，默认还会明文发送reaction。
因此，先检查缓存、再调用普通send_raw，不能作为私密审批的最终发送保证。
当前固定的0.19.1 fork增加require_encryption()；不支持加密或房间未加密时，
在消息HTTP写入前返回EncryptionRequired，显式要求加密的reaction也执行加密。
这个选项不验证收件人身份或私密成员范围，后续审批发布仍需单独完成这些检查。

Client的8项SDK补丁已固定到33de0b705ee23e8a5cb2fb8959d15b9d49e90dd7；
只更新同一0.19.1 fork的发送边界，保留原bounded history补丁和其他依赖版本。
已有加密续接消息也使用该入口。SDK修复见
[SDK PR1](https://github.com/chrislearn/matrix-rust-sdk/pull/1)，基于Client原先固定的
chris/bounded-chat-history；没有将Client升级到SDK较新的main。

实际SDK本地HTTP测试验证拒绝时消息写入为零、消息和reaction输出Megolm密文，
以及普通明文兼容：启用加密时3项通过，不启用加密时2项通过；Client使用的
bounded history组合也通过3项及严格library Clippy。未启用该history特性的严格
Clippy仍在原缓存代码报question_mark；原失败保留，没有通过allow压制。
首轮新增测试因SDK要求test_函数名前缀而未编译，修正名称后的结果单独保留。
SendPermit回归及Agent-local全量为156通过、3个原忽略。日志前缀为
/tmp/hagency-required-encryption和/tmp/hagency-remote-send-permit。

## 回归与兼容

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
产品期限。真实私密发布、双设备裁决、Native/UI和加密Agent运行仍必须另行验收。
