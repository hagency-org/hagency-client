# 当前Client/Server文件联合与真实Codex资格

## 验证范围

本批只新增隔离资格工具，不更改产品执行/授权代码。Client直接连接当前Server的
真实HTTP、PostgreSQL、Appservice和投递worker。Pasion身份、Matrix状态/媒体/消息
服务明确为loopback fixture。另经公开NativeOwner API运行实际Codex，不使用替身
provider产生该轮工具、文件或用量。它仍不替代真实homeserver、完整App SDK和桌面
人工验收，也没有生产部署、给其他人发送消息或复制provider凭据。

## 当前两仓库协议链

Server的agent_file_fixture仅允许127.0.0.1及agent_files_joint_前缀数据库，使用真实
Verifier HTTP校验建立身份，经当前Agent/Binding生命周期分配和激活。独立HTTP
Appservice事务触发真实worker；启动探针也经过真实AS回环，不伪造ready。产品
backend不挂载这些fixture路由。

Client普通联合场景使用明确的provider替身驱动实际NativeOwner执行worker：接收
131,073字节原文件，验证每个字节，追加16字节，发送131,089字节结果。覆盖实际
64KiB分块、原sender/Room/thread、实际HTTP事件回执、费用及原账本历史。正常与
已提交queue但响应丢失两种场景均实跑；丢失场景只有原audit为unknown，不因后来
sent而改写。两端重启后原历史仍可刷新，上传/消息/模型调用都不重放，费用不变。
最终日志：`/tmp/hagency-joint-wire5.log`、`/tmp/hagency-joint-lost5.log`。

这两种替身provider用量均为合成5 Token，只用于账本链路断言。不能拿它们当作
真实模型行为或实际模型费用；真实模型证据如下。

## 实际模型与公开Native入口

实际Codex CLI为0.164.0-alpha.11.1，模型gpt-6.1-sol、low、关闭搜索。独立示例
native_owner_files_qualification在进入任何Native初始化前处理真实guardian入口，
再依次调用公开设备分配、UseLocal、PrepareAgentWorkspace、保存模型、三层策略、
RuntimeStart、RuntimeStop、文件历史/刷新、审批历史和用量报表。编程及文件操作
仅在本轮隔离目录按显式规则允许，继续使用原LiveToolGate和当前设备/租约核验。
复用当前provider自有登录，不读取/复制其凭据文件。

真实模型调用receive_file和两个原生命令生成结果，再调用send_file取得原回执。
原文件131,073字节，输出131,089字节逐字节一致；原Room/thread/puppet和实际事件
一致。四项审计均为completed，观测停止范围为whole_tree。

| 实际账本/报表 | 数值 |
| --- | ---: |
| 输入 | 38,549 |
| 输出 | 401 |
| 输入中的缓存部分 | 30,976 |
| 实际花费（输入+输出，不重复加缓存） | 38,950 |
| 调用 | 1 |
| pending / unreported / reserved | 0 / 0 / 0 |

原公开入口首轮已执行并产生sent回执，但验收驱动收尾漏传bindingId，退出400；
该轮保持失败，日志`/tmp/hagency-joint-public-real1.log`。修正驱动停止请求后，通过
显式VERIFY_EXISTING只读模式验证同一原结果，没有重开模型、acquire、上传或重发。
最终核验和shutdown_checked通过；这不是一轮新的默认模式推理验收。

最终核验日志：`/tmp/hagency-joint-public-real1-final-evidence.log`；原结果：
`/private/tmp/hagency-joint-real-example1/real-codex-files-result.json`。
结果包含verifiedExisting=true、原安全文件回执、四项审计和实际费用，含义是
“原执行后续只读核验通过”，不能据此声称首个失败驱动也通过。

## 复现入口

先分别构建当前Server与Client示例：

```sh
cargo +1.99.0 build -p hagency-agent-service --example agent_file_fixture
cargo build -p hagency --example native_owner_files_qualification
```

Server需要HAGENCY_AGENT_TEST_DATABASE_URL（新的专用loopback数据库）、
HAGENCY_FILE_FIXTURE_PORT及HAGENCY_FILE_FIXTURE_INFO（私有临时JSON路径）。每个新
资格运行必须用新数据库；issuer固定在首次origin，不能更换端口后挪用旧库。
重启同一原场景保持数据库和端口不变。

Client非付费跨仓库测试：设置上述数据库、HAGENCY_AGENT_SERVER_FIXTURE_BINARY为
当前示例绝对路径，显式运行：

```sh
cargo test -p hagency native_owner_current_server_file_round_trip --lib -- --ignored --nocapture
```

另设HAGENCY_FILE_FIXTURE_LOST_QUEUE_ACK=1覆盖丢失响应场景。实际provider的公开示例
要求HAGENCY_JOINT_REAL_CODEX=1、相应fixture-info路径和全新私有
HAGENCY_FILE_QUALIFICATION_STATE，会产生provider用量。显式
HAGENCY_FILE_QUALIFICATION_VERIFY_EXISTING=1只允许原fixture目录内已有状态的后续
停止/历史/费用核验，不再启动模型或发送输入。普通CI默认不运行外部资格入口；
新增两个ignored入口与原7个忽略区分。付费libtest包装器只编译，当前真实模型证据
来自独立公开示例及原结果核验，不能把包装器的忽略当成通过。

## 保留失败与剩余工作

早期fixture错误调用cfg(test)旧分配helper、构造私有Identity和误判activate返回类型，
现只用当前生产生命周期及真实Verifier。其他编译、短HS凭据和更换端口复用旧库
所触发的部署身份拒绝保留；没有放宽产品保护。早期worker fixture的provider-home
与Room文件目录未对齐，尚无模型调用，随后修正fixture目录。显式停止后的Stopped
按原测试主人主动停止语义校验，原完成期限及文件/用量断言均保留。

首个真实libtest场景在provider检查失败，未启动原请求或产生文件；原因是libtest
入口不能提供编程guardian。已换成调用正确公开示例的包装器，保留原失败
`/tmp/hagency-joint-real1.log`。没有改成Direct进程、禁用编程或关闭进程托管。
驱动收尾400及Clippy let-and-return失败也保留，修正仅在资格工具。

Server严格all-targets Clippy、格式/拼写和OpenAPI83+4/22负向守卫通过；Client当前
引擎库261通过，原7忽略加2项外部资格忽略；严格all-targets Clippy、格式、拼写、46项守卫、一个生产caller和1161个绑定selector
均通过，missing/deferred/platformDeferred均为空。当前严格检查日志为
`/tmp/hagency-joint-client-clippy-current.log`，绑定日志为
`/tmp/hagency-joint-client-bindings.log`。b5206bf7的CI40409737074及App CI40409741968
通过，完整CI40409746628的Linux/macOS/Windows三个任务也均success，包含Windows
独立媒体恢复步骤。这些CI对应产品实现提交；本批新增资格工具的远端检查另行记录，
跨平台CI成功不等于Windows文件工具或真实Matrix联合验收通过。

下一步仍是真实Matrix/SDK、桌面人工、Windows文件工具/资格以及A4加密执行/远程
审批、A3多Agent协作和前阶段剩余门槛；A2和总体目标保持进行中。
