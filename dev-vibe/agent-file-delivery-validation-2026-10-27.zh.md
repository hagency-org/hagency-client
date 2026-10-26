# Agent 文件发送协议：当前实现与剩余门槛

2026年10月27日。Client `codex/agent-file-transfer`，已重新 fetch 并确认包含最新
main `42e59b9e`。Server 同名功能分支包含其 main `6340df32`。未直接修改 main。
A2接收已接通；本批实现 Server 发送基础和 Client 类型化设备协议。Client 动态
send_file/get_file_delivery 尚未启用，不能把接口/替身成功当作完整发送产品验收。

## Server

[Server PR7](https://github.com/hagency-org/hagency-server/pull/7) 的4f2f466及03c9e0b
实现原请求 file_outbox、prepare/chunk/queue/status、实际字节摘要校验、上传和消息
发送的分别持久化所有权，以及原可信回执确认。状态与存储契约、实际HTTP/AS测试
和原失败见 Server 的 `dev-vibe/agent-file-delivery-contract-2026-10-27.zh.md`。

上传/发送前检查新 Gateway 事实、原 owner/Agent/Room/thread/binding、执行设备/
session/lease/TTL，并在锁等待及字节哈希之后重查真实时间；未知不重新认领或重发。
迟到上传只能记录历史，不能把 unknown 变成可发消息；实际原 Matrix HTTP 回执或
HS验证的原事件可确认 sent。历史状态不含 MXC、blob、worker token，不是新发送授权。
明文 Room 上限8MiB；当前加密 Room 拒绝，A4不得通过明文降级实现。

新库串行完整103项（含全部PG）通过，0忽略；三包严格Clippy、格式、OpenAPI82+4/
19负向守卫通过。实际本地HTTP字节及AS router不是生产 homeserver 联合资格。
初始远端CI40389695253只在拼写检查拒绝测试的原三字节前缀；03c9e0b改用完整原
字节切片，实际字节/断言不变。与CI相同typos1.48本机全仓检查、该回归和严格
Clippy通过。原失败不改写，新[CI40389964561](https://github.com/hagency-org/hagency-server/actions/runs/40389964561)已在03c9e0b全部通过（镜像发布按PR规则跳过）。

## Client 类型化协议

新增安全 Manifest/Target/Delivery，匹配Server的camelCase字段和紧凑UTF-8 JSON
元组摘要。声明展示名不能作为路径；校验实际上限及小写SHA-256格式。Delivery绑定
原dispatch/execution/request、原完整声明和摘要；对后续请求另外校验原file_id。
拒绝错配、非法状态、部分已排队字节、伪造sent、已发送却没有上传接受、私有额外
字段，以及错误Matrix事件标识。支持现代不含冒号的真实opaque事件编号。

DeviceOperation新增 PrepareFile、StageFileChunk、QueueFile、FileDelivery，全部经过
既有当前Server/账号/设备固定及bearer调用。分块使用1至65536字节、无padding的
标准Base64，总HTTP131072字节上限不变，拒绝越界/空块/长度溢出。后续响应的本地
expected元数据不会提交为Server授权事实。status只提交原Target，不提交租约、
Room或URL，保留unknown；本批没有自动分块/排队/重试循环。

NativeOwner实际OAuth/设备/TCP测试验证四个原始请求体、5字节Base64、unknown
返回、错配/私有字段拒绝和最终原opaque事件回执；多次status始终只有一次prepare/
chunk/queue。此测试使用明确的合成Server响应，不证明实际Server联合或owner审批。
另测64KiB真实编码、HTTP大小、空块/超块/越界/溢出拒绝和status不带租约/目的地。

本机最终 Agent-local127通过/3个原有忽略，运行引擎258通过/7个原有忽略；两包
all-targets no-deps严格Clippy、核心格式、46项caller/spec守卫及production caller检查
通过。1155个规格绑定选择器可用（missing/deferred/platformDeferred均为空），原superseded记录保留。没有重新解释原双Agent真实资格占位失败，
也没有将App既有全仓格式漂移标为通过。

日志：`/tmp/hagency-send-client-protocol-{tests,http,agent-all,engine-all,clippy,fmt-check,guards,callers,bindings}.log`。

## 继续实现

1. 冻结选定工作目录内真实文件字节，验证原目录/父目录/普通文件身份和摘要，不跟随
   符号链接；审批等待后的发送必须使用原冻结字节，不能悄悄重读变动文件。
2. 精确三层策略/主人审批绑定原路径、展示名/caption、长度/哈希、原scope和工具call。
   本地账本先持久化一次性意图，重启/取消/快照恢复不能重新获得发送所有权。
3. 将send_file/get_file_delivery接入当前Codex动态工具、原审计和有界状态观察。
   未知只查询原id，不能因状态非终态而自动发起另一传输或声称文件失败/已送达。
4. 实际Server/Client/Matrix/模型联合、人工原生与跨平台验收；之后继续A4/A3及前阶段
   剩余资格。A2和总体目标仍进行中。


后续实现：当前macOS/Linux的冻结发送、主人审批、永久见证及动态投递工具已接通，
见 [发送工具验收](agent-file-send-validation-2026-10-27.zh.md)。上方保留类型化协议
阶段历史，不代表当前仍未实现工具；主人历史刷新和完整联合/平台资格仍待完成。

## 原请求历史查询（后续协议基础）

新增`agent-file-history-v1`、FileLookup/FileObservation和不携带lease的LookupFile
设备操作。以原dispatch/execution/request定位，prepare响应丢失也无需重做prepare。
安全响应精确核验原身份、声明及摘要；未知字段、私有媒体字段和布尔类型漂移被
拒绝。delivery=null且preparationClosed=false仍未知；true仅表示原执行不再允许
新准备，不把任何已有unknown投递当作取消，更不会返回字节写权。

配套Server0a1ef55在同一Transport事务锁内核验身份及原执行终态，只有永久不可能
回到running时给出preparationClosed。真实PG和Salvo的完整107项通过，CI40406382037
仍在执行；其前提交f5ac86c的CI40404302743已经通过。

Client当前Agent-local138通过/原3忽略，引擎260通过/原7忽略，含真实OAuth/设备/
TCP的找回记录、暂时/最终缺失、不同身份/私有字段/布尔漂移拒绝及不重传断言。
核心、App及miniapp含macos-sparkle的严格all-targets no-deps Clippy通过；首次运行
漏设Sparkle SDK环境变量而在原build.rs拒绝，保留原日志；随后指向已存在的原
固定SDK完成验证，没有改build断言或省略该feature。46项守卫、production caller、
核心格式与拼写通过。日志`/tmp/hagency-file-lookup-client-{lib,engine,clippy,
clippy-final,guards,callers,fmt,spelling,bindings}.log`。

这一步仅提供历史观察协议。主人历史列表/界面、原执行结束后的刷新以及最终
未准备证据的持久化/容量结清仍待接通，不能将类型测试算作这些功能已完成。
现有模型工具仍按原运行执行查询；没有为查历史重启模型或恢复上传权限。

当前源代码最终规格绑定仍为1155项，missing/deferred/platformDeferred均为空，
保留原superseded记录；未放宽清单上限或测试断言。
