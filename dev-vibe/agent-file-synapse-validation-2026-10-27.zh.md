# 真实 Matrix 文件收发与 v12 兼容验收

当前 Client、Server、PostgreSQL、桌面 Matrix SDK 和实际 Codex 已处理真实 Synapse
附件并取得原事件回执。本批同时修复真实环境暴露的 Matrix v12 Room ID 和创建者
权限兼容问题。身份 introspection 仍为隔离桥，未覆盖生产 Pasion OAuth 登录、
桌面人工操作、Windows 文件工具或加密 Room 执行。

## 实现修正

Matrix v12 使用不含服务器后缀的 Room hash，创建者及 additional_creators 的权限
不再取自 power_levels.users。依据[官方 v12 规范](https://spec.matrix.org/v1.17/rooms/v12/)，
Server Domain 接受43字符 URL-safe hash，同时保留旧 Room ID 和用户 ID 的服务器
校验；Gateway 只从可信当前 create 状态计算 v12 创建者权限。旧 Room 版本不能用
additional_creators 提权，坏的创建者字段或 power 字段仍拒绝，成员资格继续独立核验。

Client 的设备响应、工作区准备、项目 Room 路径和策略路径共用同一 Room ID 形状
规则，保留原大小写、原作用域和百分号编码，继续拒绝路径注入。当前 SDK 原有有界
下载与解密代码提取为共用入口，桌面 SdkTokens 和资格示例调用相同实现；它不授予
执行或落盘权限，原请求、审批、账户存活和一次性所有权检查仍在执行链中。

## 实际文件和账本

Synapse 固定为1.163.2，官方镜像摘要为
sha256:c3c21919a322f74d177a43bce8031adc81ef839ddd117a8dd03cfec350e76e02。
新容器只将HTTP发布到127.0.0.1，使用私有临时数据和本地 test 账户，不启用联邦。
Server从实际Room状态建立项目和绑定；真实Appservice回调及readiness探针通过，
媒体上传、消息发布及回执来自实际homeserver。

桌面SDK预检对131073字节原文及v2密文分别实跑认证下载；解密后每个字节一致，
篡改密文摘要会得到agent_file_integrity_failed。日志为
/tmp/hagency-synapse-sdk-preflight1.log。这个密文附件检查不等于加密Room执行。

实际Codex在公开NativeOwner编程入口中调用matrix.receive_file、两个codex.command
及matrix.send_file，生成131089字节结果。原Room/thread/Agent sender和事件一致，
读回字节逐字节一致，四项审批均completed，原停止记录为whole_tree。

| 原调用报表 | Token 或调用数 |
| --- | ---: |
| 输入 | 38892 |
| 输出 | 418 |
| 输入中的缓存部分 | 24832 |
| 输入加输出 | 39310 |
| 调用 | 1 |
| pending / unreported / reserved | 0 / 0 / 0 |

缓存已经包含在输入中。原输出事件为
$We9ZuTMHzALjOa0_8EpAfTRBkbMBay2hJGgNTx1eTKI。
原驱动在实际发送完成后用旧fixture凭据读回，真实服务器返回401，故默认驱动首轮
仍为失败。修正为SDK当前凭据后，显式VERIFY_EXISTING核验同一原记录、字节、审计、
费用及checked shutdown成功，未新增推理、acquire、上传或重发。
结果verifiedExisting=true，不能声称修正后又跑过一轮新的默认模式付费验收。

原失败日志/tmp/hagency-synapse-real3.log；成功核验
/tmp/hagency-synapse-real3-verify.log；原结果保留在私有目录
/private/tmp/hagency-files-synapse-20261027-1/real-codex-files-result.json。

## 复现入口

Server新增agent_file_synapse示例和scripts/agent-files-synapse.py。脚本要求全新
私有目录和显式本机端口；需要Docker及专用loopback PostgreSQL数据库，库名前缀
agent_files_joint_。示例需要HAGENCY_AGENT_TEST_DATABASE_URL、
HAGENCY_SYNAPSE_BOOTSTRAP、HAGENCY_FILE_FIXTURE_PORT和HAGENCY_FILE_FIXTURE_INFO。

```sh
cargo +1.99.0 build -p hagency-agent-service --example agent_file_synapse
python3 scripts/agent-files-synapse.py --root /private/tmp/hagency-files-new --port 55518 --agent-port 55519
```

Docker的host.docker.internal若选到不可达IPv6，用脚本--callback-host指定该Docker
实例的内部私有IPv4。本机实测为192.168.65.254；不能当成其他机器的固定地址。
启动Server前确认Synapse版本接口已就绪，保持原失败数据，不将旧库用于新运行。

Client构建并运行native_owner_sdk_files_qualification，设置上述bootstrap/info路径
及全新的HAGENCY_FILE_SDK_STORE。HAGENCY_FILE_SDK_PREFLIGHT=1只跑SDK原文/密文预检。
实际编程另需HAGENCY_JOINT_REAL_CODEX=1和全新HAGENCY_FILE_QUALIFICATION_STATE，会
产生真实provider用量。两个示例均在任何owner初始化前处理真实guardian入口。
VERIFY_EXISTING只做原状态核验，不能重建一次性发送或恢复原执行。

```sh
cargo build -p hagency-app --example native_owner_sdk_files_qualification
```

普通CI只编译外部资格工具，不自动使用本机账户或发起付费推理。结束后停止本轮
Server、专用PG和指定Synapse容器，保留数据/日志；不操作其他任务的容器或账户。

## 回归与保留失败

Server两个v12回归先失败再修复；完整110项（含全部PG场景）通过，严格all-targets
Clippy及OpenAPI83+4路由和22项负向守卫通过。Client引擎263通过，原7项忽略与2项
外部资格忽略分开计数；App620通过、原2项忽略保留，完成标记负向回归1项通过。
当前core/App严格all-targets Clippy（默认功能）、core格式/拼写、46项守卫、一个
生产caller及1163绑定selector通过，missing/deferred/platformDeferred均为空。
新增SDK示例单独构建和实际运行。原有全App格式漂移保留，未批量改写无关文件。

首个Server拒绝现代Room ID；修正后真实readiness曾因Docker回调IPv6不可达而失败，
现用实际内部IPv4完成原HTTP回调，没有伪造ready或放宽产品截止时间。重启Synapse
未等就绪导致的连接失败保留。第一个Client工作区入口在旧ID校验处拒绝，无执行
事件；第二个驱动把原用户附件正文中的完成标记误认成Agent回复，原请求pending、
模型调用和费用均零，未重跑该请求。驱动现在同时核对原Agent sender、Room和文本
类型，并有针对原用户正文和文件消息的负向回归。各失败使用独立库/状态保留，
后续场景是新Room和新原请求，不改变旧结果或重建旧发送权限。

默认驱动401与后续原结果核验分开记录。完整Pasion OAuth、桌面人工、Windows文件
工具和A4加密执行/远程审批、A3协作及前阶段剩余门槛仍需完成；总体目标保持进行。
