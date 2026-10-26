# 停止请求检查与显式继续实施验收

2026 年 10 月 27 日，`codex/agent-capabilities`，接续 [审批审计](owner-approval-audit-validation-2026-10-27.zh.md)。本批接通 A6 的原生恢复产品入口；整体 A2–A7 仍在进行，真实加密房间/远程裁决、桌面人工与跨平台门槛继续保留。

## 实际行为

My Agents → Recovery 可分页选择本设备的 unknown/failed 请求。停止该 Agent 服务后，检查原始输入、请求者、Room、话题、execution、身份摘要、原工作目录、已观察的停止范围、保留回复及指定文件的当前摘要。原生目录在启动 provider 前绑定原执行，停止记录只在原 provider stop 成功后保存。编程任务需整树停止；仅有 leader 停止或缺少原停止记录不能发起编程继续。

现有终态压缩会清除原输入。本批在压缩 unknown/failed 前保留其原始输入，原终态/去重身份不变。旧账本中已压缩的内容无法恢复；原工作目录或停止证据缺失时明确显示缺失，不根据当前配置推造。文件观察是当前路径的读回，不是对原任务执行成功的证明。

主人填写检查备注后可以：

1. **保持阻塞**：追加独立主人处置，原请求仍不重新执行。
2. **记录主人确认结果**：保存主人的人工结论；机器原结果、费用及审批不改成成功。
3. **检查新的继续请求 → 确认处置**：用当前 SDK 主人身份向原 Room/话题发送一条新指令，明确提及当前 Agent。新请求经过正常的 Server 身份、Room、策略、认领及费用处理，不冒充原请求者。服务停止时消息可先排队；界面提示主人启动服务。消息已接收不表示模型执行完成。

处置绑定精确检查 ID/摘要。检查有效期五分钟，处置前重读所选文件；变化或过期要求重新检查。一次检查只允许一条新处置，确认后界面要求重新检查。操作编号和原输入持久化用于精确重读；已记录的 continue 不会重新取得发送权，跨设备恢复也不重新发送。

发送可能发生之前，原子持久化 send_possible 及决定设备；SDK 使用固定 Matrix 事务编号。成功只记录 SDK 返回的事件 ID；丢响应、超时、取消、进程重启或未决恢复保持 unknown/send_possible。原操作重试仅返回原记录。主人在新的检查中明确创建另一个请求前，应核查原房间和投递记录。

## 文件及本地边界

- 最多 32 个相对路径；单文件 8 MiB，总读回 32 MiB。使用 cap-std 目录能力、末级不跟随符号链接、非阻塞打开及常规文件检查。拒绝绝对路径、父目录、Windows 流/设备写法及逃逸链接；缺失、不可读、非普通文件、过大和读中变化分别保留观察状态。
- 只保存路径、长度和 SHA256，不把文件内容发送给模型或 Matrix。检查未覆盖的文件不视为已验证。原输入预览 16,384 个 Unicode 字符，提供完整原输入复制。
- AgentHistory 恢复操作是进程内主人/SDK/设备范围入口，无新公开 HTTP 路由。继续前重新核对当前 Agent 主人、状态、原 Room 绑定和 SDK 账号。owner_direct 必须拿到加密 Room；实际加密投递仍属于 A4 的独立真实账户验收。
- 执行目录、停止记录、原输入、检查和处置随完整账本一起备份。已有 A7、A5 和审批审计版快照迁移空恢复表，不编造旧输入或原停止结果；恢复 send_possible 记录不能重获发送权。

## 验证证据

| 验证 | 已观察结果 |
| --- | --- |
| Agent 完整测试 | 113 库测试、3 programming、8 runtime 通过；3 项既有 ignored。含 105 条停止请求分页、文件改变/过期/逃逸拒绝、整树证明要求、同 ID 不重送、不同 ID 不复用检查、快照后不重送及已发布审计快照迁移。`/tmp/hagency-recovery-local-release.log`。 |
| 引擎完整库 | 252 通过，7 项既有 ignored。`/tmp/hagency-recovery-engine-final.log`。 |
| 真实原 provider 子进程链 | 既有实际 TCP + fake provider 子进程的 unknown 后新任务测试新增原停止/目录/原输入断言，实际停止返回后才能观察到对应记录，费用仍 5/0。`/tmp/hagency-recovery-observed-stop.log`。 |
| NativeOwner HTTP/磁盘恢复 | 实际本地 OAuth/TCP 授权、磁盘账本、原 Scope、文件 SHA 和设备处置；模拟 SDK 接收后失联，精确重试只调用发送一次，原 unknown 不变。`/tmp/hagency-recovery-native-flow.log`。该发送边界是显式测试替身。 |
| 实际 Matrix SDK | 私有本地 HTTP homeserver fixture 经真实 SDK session/sync/send，确认固定事务号、原 Room/话题、Agent 提及和原指令字节；账号改变及要求加密却得到明文 Room 都不产生新发送。`/tmp/hagency-recovery-sdk-flow.log`。这不是生产 homeserver 或加密投递资格。 |
| 完整桌面库 | 617 通过，2 项既有 ignored。含注册控件、原检查/路径/Agent/操作 ID 隔离、必填备注、冻结确认及 SDK 发送测试。`/tmp/hagency-recovery-app-final.log`。 |
| 既有备份入口 | 原生真实 HTTP/OAuth/磁盘快照测试通过。`/tmp/hagency-recovery-ledger-http-release.log`。 |
| 静态检查 | 三包 all-targets no-deps 严格 Clippy；Rust 格式、空白；i18n 2471 条/1863 调用/0 缺失；production caller wired1/1 全通过。`/tmp/hagency-recovery-clippy-release.log`、`/tmp/hagency-recovery-format-release.log`、`/tmp/hagency-recovery-callers.log`。 |

初次恢复测试暴露原输入已压缩，修复为压缩前保存独立原输入，而不是放宽检查；初次编译发现 SDK 回执包裹字段和 UI 可变借用问题，按实际类型及借用边界修正。没有关闭警告、自动重试未知请求或放宽原超时。

## 仍需完成

完整原生窗口的人工恢复操作、真实已加密 owner_direct SDK → Server → 新 Agent 执行的联合验收，以及 Windows/Linux 实机/远端矩阵。本批没有把它们记为通过，也没有宣称 A2/A3/A4 完成。后续先接入 A2 Agent 文件收发，并按 A4 的加密执行与远程审批、A3 多 Agent 协作完成整体目标。旧的原始 Windows caller-loss 失败仍保留，新的 Windows 原始诊断作业按实际状态复核。
