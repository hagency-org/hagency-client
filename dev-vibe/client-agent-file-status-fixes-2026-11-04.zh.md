# Agent 附件接收与聊天状态修复验收

2026-11-04，在官方 Client 0.6.4 源码上修复，使用原测试账号和原 Agent 连接 `https://hagency.meldry.com`。附件已完成真实接收、读取、回传及下载比对；聊天页在服务启动、跨页返回及停止后均显示正确状态，两项实机回归通过。

## 附件接收根因与修复

原接收调用在主人精确批准后，完成媒体 HTTP 请求，但因响应最终 URL 与原请求 URL 不同，被 `backend/agent_files.rs` 的完全相等检查拒绝。原代码将此失败收敛为 `attachment_receive_failed`，因此没有进入有效文件落盘。

带静态诊断的原行为构建在公网再次复现：日志为 `Agent attachment download unavailable at redirect`；审计 applying → failed，received_files 为 failed，无文件回执。Palpo 的媒体实现支持预签名对象存储重定向，客户端不能将这类合法响应一律拒绝。

修复继续使用原 Matrix SDK 的 HTTP 客户端及其重定向限制，接受服务端委派的同协议媒体响应；拒绝 HTTPS 降级及携带 URL 凭据的最终响应。跨存储端点不转发 Matrix Bearer，文件大小、声明长度、加密附件完整性及执行授权检查继续生效。新增测试实际运行两个独立 HTTP 端点，确认文件字节及跨端点认证头行为；没有使用替代文件、预览缓存或放宽工具审批来绕过故障。

模型仅能指定本次原始消息的 event_id，不能自行提供下载 URL、密钥或落盘路径。新增错误日志仅包含固定阶段和白名单诊断码，不记录媒体地址、签名参数或凭据。

## 公网附件闭环

- 账号：`@qa_064_20261104:hagency.meldry.com`。
- 原 Agent：`QA-064-中文-Agent-20261104`，`agt_01m6j1mvc2rfzkkt37w55t7bj0`，没有重建身份。
- 执行：`exec_86f18714ab657295b4795cfe29589a0ef20b63d5d8d574553549a7be7e3e8fea`。
- Agent、Room、请求者三层高风险策略均为 AskOwner；模型请求也逐项批准。
- 原附件 `qa064-attachment.txt` 为 39 字节，内容为两行 `QA064-FILE-20261104`、`中文附件测试`，含换行。
- `matrix.receive_file` 批准后 completed；实际工作区文件为 ready，长度及 SHA-256 正确。
- 单次批准 `cat` 读取该确切路径；命令审计 completed，exitCode 为 0，没有修改执行策略。
- `matrix.send_file` 批准后从同一路径冻结并回传 `qa064-roundtrip.txt`，没有创建替代内容。
- 原 delivery_id：`b75ce37a171703ae78ffdfc3b23468b261ccc4dc6c198204012aa41d2392fc35`。
- 发送审计 completed，远端状态 sent，Matrix 事件 `$oYpPXUHTpic8EU4GZ2on8F4ehRNy0A5imTmvh22HXkQ`。
- 模型最终回复确认 `get_file_delivery` 为 sent，并报告原两行文本；聊天中出现来自 Agent 的真实 39 B 附件。
- 普通客户端下载该回传到 `/Users/chris/Downloads/qa064-roundtrip.txt`，界面显示“已下载”。

原文件、工作区接收文件及回传后下载文件逐字节一致，SHA-256 均为：

```text
2a09025d312e77e5d47fae66b9b14ffb8f69f8bf1515a5f1d58ea4743838137c
```

公网闭环使用的本地构建主二进制 SHA-256 为 `ac517b2a019593d145ced9e63711f905cfe82eb118a1563b695c069b5ff66b50`；之后的修改只补全聊天状态生命周期及诊断，附件实现保持一致。

## 聊天状态根因与修复

聊天横幅独立读取本地运行状态。隐藏 RoomScreen 时，原实现清理了房间和时间线，却没有清理横幅的请求、加载标志及定时器。隐藏期间可能收不到相应计时或异步完成事件。返回同一房间后，横幅仍认为旧查询正在执行或旧定时器有效，停止文字一直保留。绘制时无条件将横幅置为 active 又会掩盖实际激活边界。

修复包括：隐藏房间时取消横幅活动状态并清空房间，使旧请求序号失效、清理 loading 和 target；绘制与事件处理共用精确房间及当前页面检查；重新激活时恢复轮询。横幅改读完整 AgentServiceStatus，保留服务启动状态，并只显示对应 Room 的运行状态和问题。

测试覆盖房间隐藏期间的在途请求失效、loading 与定时器清理、同房间重新激活、服务启动中状态、其他房间及其他 Agent 隔离、轮询时布局稳定性。最初只补重新激活的实现实机仍失败，原结果保留；最终隐藏时清理的实现需以下方实机记录为准。

最终实机记录：用户完成系统钥匙串授权后，最后构建恢复原账号。先打开原私聊，再从 Agent 列表启动服务，返回同一聊天显示“正在本设备服务全部已接入的 Room”；切到项目页再返回，同样恢复正确运行文字；从 Agent 列表停止服务后，返回同一聊天显示“本设备未运行，请在‘服务运行’中配置并启动”。列表与聊天状态一致。日志对应请求 2 为 enabled=false/runtime=false，请求 5、9 为 enabled=true/runtime=true，请求 13 为 enabled=false/runtime=false，结果均匹配当前身份、房间和请求序号。重新进入后会先隐藏旧横幅，待查询完成再显示结果。

没有绕过系统 SecurityAgent 或重置账号。收尾已确认服务未运行、编程执行 enabled=false、Agent 高风险工具恢复 Deny（revision 5），请求审批仍 AskOwner；Room/请求者的测试工具 AskOwner 保留，受 Agent 层 Deny 约束。11 条 calls 全部 settled，11 条 inbox 全部 replied，三层 held 均为 0，未新增模型任务。

## 本地验证与证据

| 检查 | 结果 |
| --- | --- |
| 新重定向测试运行原完全相等逻辑 | 失败，exit 101，保留 files-original-regression.log |
| 同房间轮询测试运行原无恢复逻辑 | 失败，exit 101，保留 status-original-regression.log |
| 最终 App 库测试 | 885 通过、0 失败、2 项原有忽略 |
| Agent-local 接收相关测试 | 9 通过、0 失败 |
| 早期未限制依赖的 strict Clippy | article-makepad 存量 27 项 lint 失败，未修改这些无关文件 |
| App no-deps strict Clippy | 最后构建通过，retire-fix-clippy.log |

完整 App 测试使用 `HAGENCY_NATIVE_CREDENTIAL_IO=deny cargo test --locked -p hagency-app --lib`；Clippy 使用 `HAGENCY_NATIVE_CREDENTIAL_IO=deny cargo clippy --locked --no-deps -p hagency-app --lib -- -D warnings`；忽略项没有当作通过。`git diff --check` 通过。没有改服务器、没有部署、没有发布新 Release。

安装位置 `/Applications/Hagency.app` 为本地修复构建，官方 0.6.4 应用保存在 `.run/public-client-qa-20261104/Hagency-official-0.6.4-backup.app`。最后构建主二进制 SHA-256 为 `a23c7d6b81d97f852b0c217a113d921f5fada919c1e2e404d9415ea31f2a497c`。这不表示 GitHub 官方 0.6.4 下载已经更新。

证据保存在忽略目录 `.run/public-client-qa-20261104/`：`diagnostic-live.log`、`redirect-original-checkpoint.private.json`、`roundtrip-success-checkpoint.private.json`、`fixed-live.log`、`status-trace-live.log`、`retire-fixed-live.log`、各轮测试与构建日志。原始状态和日志可能含认证信息，不提交；界面截图在本对话工具记录中。

本轮新增 3 条请求：1 条上传时未填入说明的请求被主动拒绝，0 模型用量；1 条原行为故障复现，125719 token；1 条成功附件闭环，504955 token。新增累计 630674 token，包含工具轮次的累计输入输出口径，缓存项不重复加算。所有 11 条历史请求均已结算，总累计 864267，held 为 0。没有继续发送付费模型任务。

跨平台、其他模型、多用户共享房间、跨设备恢复、大文件及长时间断网场景仍未在本轮验收中覆盖。
