# Client 0.6.4 公网注册与本地 Agent 验收

2026-11-04，macOS Apple Silicon，公网服务器 `https://hagency.meldry.com`。结论：新用户注册、桌面授权回调、本地 Agent 身份验证、Codex 实际回复、审批及停止重启主链路通过；Agent 接收附件失败，文件往返未通过。聊天页仍出现与实际运行状态不一致的横幅。

## 安装与测试数据

- 官方 [v0.6.4 Release](https://github.com/hagency-org/hagency-client/releases/tag/v0.6.4)，发布于 2026-11-03 11:57:39 UTC，源提交 `deddebdeaa6d510c1c37e2ada80e90b5b070e647`。
- 下载 `hagency-macos-arm64.dmg`，SHA-256 `56d2978d09ef3c853723b445a51533c66db84bc31243606942ff0baea466bf6a`；实际架构 arm64、Info.plist 版本 0.6.4。
- `/Applications/Hagency.app` 已从 0.6.3 更新为 0.6.4；原应用备份在 `.run/public-client-qa-20261104/Hagency-0.6.3-backup.app`。
- 安装后主二进制与下载应用一致，SHA-256 `02c6718092e711b14e8c9025b6c73fac2d9228ae64dbfa52106cdca0d1df091c`。139 个桌面资源均与 v0.6.4 Git 对象逐字节一致。
- 独立数据目录：`.run/public-client-qa-20261104/profile`。测试账号 `@qa_064_20261104:hagency.meldry.com`，密码由用户在浏览器完成；报告不保存密码或令牌。
- Agent：`QA-064-中文-Agent-20261104`，ID `agt_01m6j1mvc2rfzkkt37w55t7bj0`；仅本人私聊。
- 模型：本机已登录的 Codex，`gpt-6.1-sol`、推理强度 low。未导入个人 MCP。
- 项目：`QA-064-测试项目-20261104`，默认主看板，中文卡片 #1 `QA064：注册与 Agent 连通验收`，最终在“进行中”列。

当前桌面进程使用此独立目录。后续查看该测试账号需使用同一目录启动：

```sh
HAGENCY_APP_DATA_DIR=/Volumes/Data/Works/hagency-org/hagency-client/.run/public-client-qa-20261104/profile /Applications/Hagency.app/Contents/MacOS/hagency
```

## 场景结果

| 场景 | 结果 | 实测依据 |
| --- | --- | --- |
| 全新资料目录启动、中文界面 | 通过 | 无保存账号启动，切换中文成功 |
| 空服务器地址、SSO 取消 | 通过 | 空地址拒绝，取消可返回服务器页 |
| 公网发现、健康、就绪 | 通过 | discovery、healthz、readyz 返回 200；startupRoundtripConfirmed 为 true |
| 注册入口、OAuth 回调 | 通过 | prompt=create 注册表单，用户完成凭据，loopback 授权回调后桌面登录成功 |
| 未登录及无效令牌边界 | 通过 | identity、agents、projects、devices 返回 401；无效 Bearer 401；原生 API 携带浏览器 Origin 为 native_origin_required；Matrix whoami 为 M_MISSING_TOKEN |
| Agent 名称校验与创建 | 通过 | 空名拒绝，中文名创建成功 |
| Agent 加密身份 | 通过 | 原身份达到 keys ready，核对完整身份后验证成功；没有替换身份来绕过失败 |
| 私聊接入、本地模型保存和运行 | 通过 | 本地设备绑定，Codex 模型保存，服务实际执行 |
| 固定标记与上下文 | 通过 | 实际回复 QA064-PONG-20261104；下一条重复标记并输出“中文与 Markdown 测试通过” |
| 审批允许 | 通过 | offered → owner_allowed → applying → completed，实际回复 APPROVAL-FRESH-OK |
| 审批拒绝 | 通过 | owner_denied，明确未调用模型，0 token 结算 |
| 审批过期 | 通过 | 原请求 300 秒后 expired，明确未调用模型，0 token 结算；最初未产生提交记录的点击不计为批准成功 |
| 工具策略拒绝 | 通过 | Agent Deny 时拒绝；仅 Agent AskOwner、Room 和请求者仍为默认 Deny 时也拒绝，无文件写入或回传 |
| 单次预留额度为 0 | 通过 | 启动被“请输入大于零的 Token 预留额度”拒绝，恢复为 10000 |
| 项目创建和看板 | 通过 | 空项目名拒绝；中文项目和任务创建；卡片状态修改保存、冷启动后可见 |
| 正常停止与冷启动 | 通过 | 会话恢复，历史可解密，停止的服务保持未运行；startup recovery 为 0 eligible bindings，无原两条任务重放 |
| 附件上传和普通客户端下载 | 通过 | 原始请求附件显示 39 B；下载后长度和 SHA-256 与原文件一致 |
| Agent 接收附件 | **失败** | 三层均 AskOwner、精确工具批准后 attachment_receive_failed |
| Agent 读取、send_file 回传 | **未通过前置条件** | 接收失败，未读取内容，sent_files 为 0，不算文件往返成功 |
| 审批历史和用量展示 | 通过本次记录核对 | 工具失败历史与审计一致；界面已用 233593、预留 0，与账本一致 |

## 需要修复或继续定位的问题

### Agent 附件接收失败

复现条件：新账号、已验证的私有 Agent、同一私聊、本地 Codex 编程执行开启、MCP 空集；Agent、Room、请求者三层高风险策略均为 AskOwner。发送 39 字节文本附件，批准模型请求，再核对并批准 `matrix.receive_file`。

2026-11-04 01:47:37 UTC 提出工具请求，01:50:09 主人批准，01:50:14 applying，01:50:19 failed，原因 `attachment_receive_failed`。对应执行 `exec_4a43c4ee126fec3d39005e2caabc3ac6c8eaedfa2d5726942e0245de9bc16318`。`received_files` 最终 failed、receipt 为空；`sent_files` 无记录。模型最终如实报告失败，没有虚构文件内容或声称回传成功。

同一条消息的附件由普通客户端成功下载到 `/Users/chris/Downloads/qa064-downloaded-20261104.txt`。内容为 `QA064-FILE-20261104` 和 `中文附件测试`，包含换行，共 39 字节；SHA-256 `2a09025d312e77e5d47fae66b9b14ffb8f69f8bf1515a5f1d58ea4743838137c`。这将故障范围缩小到 Agent 接收链路，不能据此认定具体是服务器、下载授权、解密或落盘问题。

源码 `crates/hagency-agent-local/src/codex/received_files.rs` 将下载/授权失败及部分发布前错误收敛为此错误码；`crates/hagency-app/src/hagency/backend/agent_files.rs` 承担有界媒体下载和解密。本次原始日志没有给出可唯一定位的下层错误，没有修改代码或用替代文件绕过。

### 私聊运行状态展示不一致

重启后多次进入通讯录私聊，顶部仍显示“本设备未运行”并有刷新指示，同时“我的 Agent”列表显示“运行中”，该 Agent 也成功完成新模型请求和审批。实际执行成功，横幅状态未随之正确显示。0.6.4 的实测仍出现此问题，不能用已有源码回归记录代替发布包验收。

“打开服务”导航曾落到 Agent 标签，部分自绘控件第一次点击只获得焦点；这些交互需另行复核，未据此认定模型审批功能失败。空地址说明、注册后部分 Agent 提示和下载提示仍混用英文。

### 初始化日志噪音

新账号有 `user data not found` / key backup 不存在的 404，Agent SDK recovery 出现 `agent_crypto_operation_not_allowed` 401。之后原身份就绪、验证及真实任务成功；保留这些错误，不归为本次登录或身份主链路失败。历史分页及一次临时解密告警未阻止已验证的历史阅读，原因未确定。

## 用量与最终状态

账本有 8 条 calls，全部 settled；8 条 inbox 全部 replied。6 条实际模型任务，2 条审批拒绝/过期任务均无模型用量。累计 input + output 为 **233593 token**，其中 cached input 为 207616，reasoning output 为 90（缓存和推理子项不再重复相加）。Agent、Room、请求者三层记录相同消费，不能把三层再求和。该统计采用 Codex app-server 累计口径，不换算金额。

附件正向任务本身累计 input 155462 / output 620，包含工具交互轮次；失败也产生模型用量。最终 held 为 0，pending calls 为 0，待审批为 0。

测试结束已停止 Agent 消息处理，界面确认“未运行”；保留账号、Agent、项目与消息供复查。Codex 编程执行恢复 false，Agent 高风险策略恢复 Deny，模型请求仍 AskOwner。Room/请求者的测试 AskOwner 策略保留，Agent 层 Deny 仍阻止高风险工具。未删除身份、未注销账号。

## 覆盖边界与证据

本次未覆盖 Windows/Linux/Intel Mac、Claude/Octos、多人共享房间、邀请与成员权限、远程审批、跨设备密钥恢复、多 Agent 并发、长时间断网和故障恢复、完整预算耗尽及硬额度压力测试、所有文件类型及大文件。Agent 回传文件被接收失败阻塞。未执行无关的源码单元测试来代替发布包实测。

私有证据位于忽略目录 `.run/public-client-qa-20261104/`：Release 元数据、DMG 和哈希、资源校验、服务发现与未授权请求检查、两次进程日志、分阶段审批账本、最终账本 `final-ledger-checkpoint.private.json` 及独立 profile。原始日志和 profile 含认证状态，不应提交或对外分享；界面截图保留在本次对话工具记录中。

工作区没有 provisioned `task-writer`，无法写入模板所指的控制平面；此文档及 docs 仅记录验收结果。未修改业务代码、未提交 Git、未发布或部署。

## 后续修复

原发布包失败记录保留。随后本地修复构建已通过附件接收、读取、回传、下载字节比对及聊天状态切换回归，详见 [Agent 附件与聊天状态修复验收](client-agent-file-status-fixes-2026-11-04.zh.md)。该结果不表示官方 0.6.4 发布包已经更新。
