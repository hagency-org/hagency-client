# Agent 冻结文件发送与原投递查询

2026年10月27日，`codex/agent-file-transfer`，已重新 fetch 并确认包含 Client main
`42e59b9e`。本批将 send_file/get_file_delivery 接入当前 Codex 原生运行链；A2仍
在实施和联合验收中，没有部署。Server 依赖 [PR7](https://github.com/hagency-org/hagency-server/pull/7)，
其03c9e0b的CI已通过。此前类型化协议报告保留为历史，见
[发送协议](agent-file-delivery-validation-2026-10-27.zh.md)。

## 实际行为

当前账号/SDK、Server元数据及文件投递能力可用，主人显式启用编程或Room文件工具，
且执行端为macOS/Linux时，当前Codex动态目录增加：

- `send_file {path, filename?, caption?}`：只接受选定工作目录内的相对普通文件，
  冻结真实字节后申请精确策略/主人审批。目的Agent、Room/thread、执行设备与租约
  由原请求决定，模型不能提供替代URL、Room、设备、字节或Server请求编号。
- `get_file_delivery {delivery_id}`：只查本执行已登记的原本地投递编号。没有上传、
  续传、换编号或重发功能；sent及实际Matrix事件回执才证明消息投递。

文件单个8MiB，最多4份活跃冻结字节；本地账本最多4096条永久见证，未结声明字节
预留128MiB。账本不保存文件blob，快照也不复制工作目录文件。known sent/cancelled
可以结清声明字节预留，未知记录不被自动清理为“未执行”。

## 冻结、审批和不可重建的所有权

从实际保留的工作目录对象逐层nofollow打开父目录和普通文件，拒绝绝对/点目录/
符号链接、硬链接、特殊对象、非当前用户所有及超大文件。读取前后检查原文件身份、
长度、mtime/ctime，以及原目录/父目录/文件对象。允许普通0644工作文件；不套用接收
私有0600发布文件的限制。

提案绑定原scope/dispatch/execution、Codex thread/turn/call、真实相对路径、展示名/
caption、实际长度/SHA-256和manifestDigest。等待主人期间持有原字节和物理对象；
批准后重新检查原文件/目录、原运行执行和当前策略。源文件或目录已变则不开始IO。
取得所有权后只发送已批准的冻结字节，后续源文件变化不会悄悄改变上传内容。

只有首次提交sent_files见证的调用者可以获得不可克隆的内存发送所有权。见证在
Server prepare/chunk/queue之前落库；旧调用、新工具call编号、重开账本或快照恢复
都不能从该记录重新构造所有权。同一原请求未结的相对路径或内容摘要也不能通过
换展示名、复制文件或换工作目录来清除未知。已知相同原文件直接返回原历史，不再
提示或发送。已拒绝的同一工具call返回原拒绝，不重复弹窗或重新应用。不同真实输出文件仍可在本次执行内分别发送。

宿主逐次复核当前原执行、设备、租约、Matrix权限和本地策略，使用原Server接口
分块发送64KiB，每个调用保持128KiB通用HTTP上限。排队后最多进行十次、间隔1秒的
原记录只读观察，不延长原审批/执行期限，不重新提交。未证实的步骤在审计中保留
准备、分块、排队或观察阶段的固定原因，不把网络失败解释成远端不存在。

查询前后检查当前权限和本地请求拒绝；中途撤权不能把历史结果返回给模型。真实
历史回执可以留在主人账本。原发送审计的unknown不因迟到sent而改写，原派发的
unknown/费用也不被更正成成功；文件实际投递状态是独立证据。

## 接入与兼容

当前NativeOwner先发现Server能力并绑定原执行，然后为当前Codex上下文附加工具。
未满足条件不启用发送工具。沿用既有主人审批队列和原审计页面，不恢复退役Fleet。
新的`hagency-agent-files-v1`上下文与旧纯接收/普通上下文分开；结算接受精确格式，
未知用量清理同scope的累计基线，保留其他scope。受限文件和编程两种前缀均有回归。

Schema在所有平台一致保存发送见证，实际捕获/工具执行当前只在Unix接通。接受精确
已知旧快照Schema，并添加空发送表；保留现有接收、恢复、审计和费用记录。当前
快照保留未知发送及原投递编号，不凭恢复行为生成新授权。

Server当前仅允许明文Room；加密Room即使已上传也拒绝后续消息发送，不能降级。
真正加密Agent执行、私密审批和加密文件发送继续属于A4联动目标。

## 本机证据

Agent-local137通过、3个原有忽略；programming3和runtime8通过。运行引擎260通过、
7个原有忽略；App618通过、2个原有忽略。两核心包及App严格all-targets no-deps
Clippy、核心格式、46项caller/spec守卫及production caller检查通过。规格绑定结果
记录于本文件末尾。没有将未覆盖的真实资格或App既有全仓格式漂移改写为通过。

真实磁盘测试覆盖源/父目录/工作目录替换、链接/特殊对象/超限拒绝、冻结字节在源
改变后不变、精确审批和撤销、4份活跃内存限制、实际16份8MiB未结预留达到128MiB
后拒绝第17份、永久见证、重开/恢复后不重建，以及原迟到回执和历史禁止倒退。
当前动态工具的真实owner队列覆盖批准、拒绝、审批中改源/撤权、不同call编号不重发、
丢失排队返回、迟到sent保留原unknown审计和状态读取中本地策略撤销。

NativeOwner真实OAuth/设备/TCP/账本/工作目录/运行宿主测试：合成provider从本次动态
目录调用发送和查询，验证实际5字节编码、原声明、一次排队、正常/丢失排队返回、
原事件回执、原回复/重启不重跑、租约续期及真实5 Token结算。provider和Server回执
明确是替身；这不是实际Server worker/Matrix homeserver/真实模型/人工桌面联合验收。
Server真实PG/HTTP/AS路径另在配套报告验证，不能用独立通过替代整条联合路径。

日志：

- `/tmp/hagency-send-release-libraries-final.log`
- `/tmp/hagency-send-engine.log`、`/tmp/hagency-send-release-native-final.log`
- `/tmp/hagency-send-app.log`、`/tmp/hagency-send-app-clippy.log`
- `/tmp/hagency-send-release-clippy-current.log`
- `/tmp/hagency-send-release-{guards,callers,fmt-check,bindings,spelling}.log`

原失败保留：新测试辅助函数最初缺少导入/测试时钟（capture-tests），快照fixture
错误地按一列插入实际两列表（capture-tests-fixed），新TCPfixture比较引用和值造成
编译失败（native），以及Clippy要求折叠if（tools-clippy）。另一个完整回归拒绝了
“删接收表却留未来发送表”的伪旧Schema（libraries）；现仅修正旧Schema测试构造，
同时移除后来的空发送表，原严格验证和原断言不变。没有加重试、放宽超时或隐藏
失败。首个实际执行的Native发送宿主回归中，两种场景均在原每轮20秒宿主上限内通过。

## 仍须完成

1. 主人对已结束/停止执行的历史投递查看和刷新，取原Server状态结清真正的sent/
   cancelled预留；当前模型工具严格限定原运行执行，不能为查询而重启旧模型。
2. 真正Server、当前Client、实际Matrix和真实模型联合文件路径及人工桌面验收。
3. Windows文件工具实现/资格及其他跨平台门槛；A4加密执行/文件/远程审批、A3多
   Agent协作和前阶段剩余资格继续在总目标内。A2和总体目标均未完成。

此前接收1bbff570的完整Linux/macOS/Windows矩阵40383538746已通过；b2d9b14d的App CI
也通过。这不代替本批新发送代码的远端检查，后续推送将另行运行。


最终日志：`/tmp/hagency-send-final-libraries.log`、`/tmp/hagency-send-final-native.log`、
`/tmp/hagency-send-final-clippy.log`及`/tmp/hagency-send-final-{fmt-check,spelling}.log`
确认同一拒绝调用的去重、查询中本地拒绝和最终源代码检查。规格绑定1155项、
missing/deferred/platformDeferred均为空，原superseded记录保留。


收尾补充：普通结束回复另附本轮未确认/已取消文件数；只读取经过校验的原本地
投递回执，不把模型文本、排队或上传当成已送达。Native宿主第三种持续unknown
场景验证提示、费用与重启不重发，原每轮20秒上限不变；日志
`/tmp/hagency-send-notice-{libraries-final,engine-final,clippy-final}.log`，引擎最终260通过。
Server另修复“任务已完成后已观察撤权又恢复”的旧文件再认领问题，新增永久原事件
否定日志，并验证旧03格式数据库全部记录摘要不变及全套106项通过；原失败记录
在配套Server报告中。当前发送依赖包含该修正的PR7提交f5ac86c；其新CI40404302743
仍在执行，旧提交03c9e0b的通过结果不代表此修正已完成远端验证。

最终源代码的规格检查：`/tmp/hagency-send-current-bindings.log`中1155项绑定，
missing/deferred/platformDeferred均为空；`/tmp/hagency-send-current-guards.log`中
46项守卫通过，current-callers记录1项wired且无缺失；current-fmt和current-spelling
均无错误。推送前重新fetch了Client/Server的main，两者仍为42e59b9e/6340df32，
均是当前功能分支祖先，没有直接修改main。

后续历史刷新必须处理prepare响应丢失、尚无远端file_id的情况：按原request_id
只读查询，不能再次prepare或恢复字节写权。暂时404不证明没有投递，只有原执行
终止并经Server序列化确认的最终不存在证据才能另行结清容量，不能伪造cancelled
回执或改写原unknown审计。
