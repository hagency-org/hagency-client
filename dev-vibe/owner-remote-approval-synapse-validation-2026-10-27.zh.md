# 真实远程审批联合验收

当前私密审批链已经通过真实Synapse、两个独立桌面SDK、两个公开NativeOwner和
实际Codex的联合验收。任务本身仍在普通Room执行；本结果不代表Agent加密身份/
任务/文件执行已完成。生产Pasion OAuth和人工桌面操作也没有由此得到验收。

## 实际结果

使用全新隔离Synapse1.163.2，镜像固定为
sha256:c3c21919a322f74d177a43bce8031adc81ef839ddd117a8dd03cfec350e76e02。
服务只发布到127.0.0.1，无联邦，两个SDK拥有各自的实际Matrix设备及SQLite存储；
两个NativeOwner也有不同的执行设备身份。真实Server/API/PG/AS/worker接收原任务。

通过SDK公开cross-signing和设备verify API签名另一台设备，比较两边实际公开master
key后，在隔离主人的远端SDK确认相同身份。没有导入伪造Olm会话、设置LocalTrust
或构造Verified元数据。新审批室只有主人一个成员，状态为invite、禁止guest、
joined历史可见性和Megolm加密。SDK实际密码学及当前私密状态检查均通过。

执行设备配置三层AskOwner后，实际Codex提出原命令审批。原生发布固定提案；另一台
NativeOwner经真实SDK读出相同原提案和事件，持久预留一次决定，再用真实SDK加密
发送。决定已经取得原投递回执时，文件仍不存在。执行设备按原关系集合自动发现并
验证决定，交给原活跃PendingApproval，随后实际终端命令才执行。

输出文件的31字节与预期逐字节一致。原audit为completed，OwnerAllowed事件记录
verified_matrix_device来源，原请求/决定事件与真实SDK回执一致。实际whole_tree
停止已落账，checked shutdown成功。Server验后保留一个历史租约epoch行，当前
live lease计数为0；不能把保留行说成删除，也不声称停止瞬间就移除了租约行。

| 成功原调用 | 数值 |
| --- | ---: |
| 输入Token | 10839 |
| 输出Token | 73 |
| 输入中的缓存部分 | 5248 |
| 输入加输出 | 10912 |
| calls | 1 |
| pending / unreported / reserved | 0 / 0 / 0 |

缓存已包含在输入中。真实请求事件为
`$_aZyR050p75Xl-QvvystXmYSlHVuGiJXAtGemIwMQ5c`，决定事件为
`$HVOSJ4R94scV4OfcDDAE9ZMK4R5qOGuV1dxySHF7sVE`。独立只读HTTP检查确认两者均为
m.room.encrypted、m.megolm.v1.aes-sha2，Room/sender匹配，原命令不出现在明文事件
JSON中。两台真实SDK token均通过隔离身份桥；Bob、无效token和错误client secret
均被拒绝。这个桥仍然不是生产OAuth实现。

## 复现与保留证据

Client示例native_owner_remote_approvals_qualification在任何初始化前处理实际guardian
入口，并复用与桌面SdkTokens完全相同的有界SDK函数。公开入口返回的证明不是工具
授权；原账本、活跃审批和当前Server/执行设备/租约/策略检查仍由NativeOwner持有。
新增shlex仅为dev-dependency，解析实际Codex的`/bin/zsh -lc`封装，要求其完整payload
与原命令一致；额外命令、参数、不同输出路径及坏引号均被资格守卫拒绝。

Server原有隔离脚本新增显式--retain-owner-password，仅为新test账户在私有目录保存
多设备登录所需密码，默认不保存。示例introspection桥逐token调用真实whoami，支持
两台独立主人设备而拒绝其他用户；生产router、认证协议和数据schema均未改变。

需要全新私有fixture目录、SDK/owner子目录及前缀agent_files_joint_的专用loopback
PostgreSQL库。Server示例使用HAGENCY_SYNAPSE_BOOTSTRAP、HAGENCY_FILE_FIXTURE_INFO和
HAGENCY_FILE_FIXTURE_PORT；Client另需HAGENCY_REMOTE_APPROVAL_ROOT及显式
HAGENCY_JOINT_REAL_CODEX=1，会产生实际provider用量。普通CI只编译示例并跑无网络
命令守卫，不自动使用本机登录或执行付费推理。

本次成功证据目录为
/private/tmp/hagency-remote-approvals-synapse-20261027-4/joint4，保留result.json和
transport-proof.json；成功进程日志/tmp/hagency-remote-synapse-joint4.log。当前原生
设备和SDK已关闭，本轮Server、专用PG及四个隔离Synapse容器均已停止，数据未删除。

## 原失败不改写

前三轮使用互不相同的新服务、数据库和状态目录，没有重新发布旧请求。

- joint1在模型启动前失败：普通SDK密码登录跟随Synapse的well_known地址，绕过
  测试用共站身份桥，被现有SDK账户地址检查拒绝。改为把真实Matrix登录响应恢复到
  固定代理地址的新SDK，保留原账户检查；该轮没有NativeOwner目录或provider调用。
- joint2实际加密和两台NativeOwner均建立，但资格脚本给Agent级设置多传binding
  参数，被正常接口拒绝；两个账本的calls/audits均为0。仅修正资格命令参数。
- joint3实际模型正常回复“codex.command不可用”，无工具调用和文件。脚本误将内部
  审计分类当成提供给模型的工具名，等待不到审批而超时。该调用已settled：10848输入
  +119输出=10967，缓存5248为输入子集；whole_tree停止。它不是远程审批通过，结果
  及用量原样保留。joint4使用独立新任务，改为请求实际可用终端，不重跑joint3。

最初Server示例修改的header借用、Client示例reqwest JSON feature/device_id参数编译
失败也保留在各build1日志；修正使用现有bytes/serde接口，没有扩大依赖特性。

## 当前回归和剩余范围

实际新CLI构建和单项命令守卫通过，当前App默认功能all-targets严格Clippy使用正式
--no-deps边界通过；远程相关App15项通过。Server示例严格Clippy、Python语法检查及
真实身份桥正反检查通过。锁文件只新增既有shlex2.0.1的App开发依赖边，没有其他
依赖升级。此轮未重复声称全Server110项或整个Client平台矩阵已经验过新提交。
当前core格式/拼写、46项静态守卫、一个生产caller和1191个契约selector通过，
missing/deferred/platformDeferred均为空；bindings-final和static-final日志单独保留。

此前a7d08a7c快速CI40440897177及拼写通过；ff496b83 App40438410171的两个macOS
架构和article job通过，full40438413947的Linux/macOS/Windows实际job均已通过。
这些源版本与当前资格工具提交分别记录。

下一步继续当前Agent独立加密身份、加密任务/回复/文件及A3多Agent协作；保留所有
早先平台、人工、生产OAuth门槛。A4和总目标仍在进行中。
