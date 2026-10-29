[English](hagency-quickstart.md) | [简体中文](hagency-quickstart.zh-CN.md)

# 原生 Hagency 快速开始

本文对应当前 Projects/Agents 原生页面。Palpo 小程序、Fleet／服务器关联／
资源申请以及协调者审批流程已退休。

## 一次登录

在登录页输入 Hagency 服务器地址，在浏览器完成 Pasion 登录和授权。服务器
必须声明受支持的 Hagency 协议；普通 Matrix 服务器会被拒绝。Matrix SDK 负责
会话刷新和保存，原生管理复用当前账号授权，不要求复制 token 或再次登录 Matrix。

Pasion 声明支持注册时，选择服务器后还会显示**创建账号**。点击后通过 OAuth
`prompt=create` 在浏览器进入 Pasion 注册；完成注册和授权后，SDK 回调会登录
Desktop。服务器关闭注册时，这个入口隐藏。

登录页及 **Settings → Account** 可选择已保存账号。切换会停止当前主人的本地
Agent 运行并清理活动权限；原账号的独立数据保留，不转让 Agent 所有权。

## 创建 Project 与讨论 Room

打开 **Projects → New Project**，也可以从侧栏 Projects 的 **⋯ → New project**
进入。默认新建 Space，也可以连接当前账号已经加入的 Space。候选分页或局部
查询失败需要明确处理，不把不可用候选当作可选项。

Project 是 Hagency 中绑定 Space 的记录，不会把每个普通 Matrix Space 都自动
当作 Project。名称和主题来自 Space。详情提供四个区块：

| 区块 | 用途 |
| --- | --- |
| Overview | Space 身份、说明、Open Space。 |
| Rooms | 新建 Room，或接入已关联该 Space 的 Room；打开聊天和成员。 |
| Members | 查看真实 Space 成员；有 Matrix 邀请权限时邀请成员。 |
| Access | 查看接入权限与服务状态；有管理权限时才能编辑。 |

Space 成员不会自动复制到所有 Room。每个讨论 Room 有独立成员与邀请规则。
目录中 Project 条目的 **⋯** 还可直达概览、讨论 Room、成员、邀请成员和 Space。
创建结果未确认时，使用 **Resume original request**，不要猜测结果后重复新建。

Project/Room 接入策略决定哪些成员可以把 Agent 接入此范围，禁止名单优先。
它不审批 token 配额。**Service pause** 是独立设置，可暂停已有服务；解除管理员
暂停不会自动恢复主人已暂停的绑定。普通成员不能修改这些管理设置。

## 创建服务器级 Agent

打开 **Agents → New Agent**，填写显示名称并创建。Agent 永久归创建者所有，
默认执行设备就是当前已授权设备，不要求先选择 Project 或讨论 Room。等待身份
配置完成；主人私聊会按原请求设置。必要时使用 **Set up / retry private chat**，
就绪后 **Open private chat**。

只有指定执行设备可以运行这个 Agent。**Handle on this device** 经显式确认及
版本检查后改为当前主人的设备，不转让身份、不复制预算、不清零已消耗 token，
也不授权重跑未知执行。新设备开始新请求前仍可能需要恢复同一主人的完整账本。

讨论 Room 接入是另一步：使用 **Add Room binding**，或在讨论 Room 聊天信息的
**Members → Invite my Agent** 中选择已存在 Agent。后台仍校验 Project/Room
接入权、真实成员状态与傀儡邀请权限；成员选择器不会另建一个 Agent。

## 四个 Agent 详情标签

| 标签 | 配置与查看内容 |
| --- | --- |
| Agent | 身份、执行设备、主人私聊、Room 绑定、暂停／恢复、离开与永久退役。 |
| 模型与资源 | 本机 Codex／Claude Code／Octos、模型或 profile、推理选项及 Agent 工作目录。 |
| 限制 | Agent 总额、选中 Room 总额、该 Room 内选中请求者。 |
| 服务运行 | 选中 Room 运行、估算预留、受限文件工具及待审批请求。 |

在“模型与资源”选择 **Local agent runtime**：Codex、Claude Code 或 Octos。
选择 **Connect my local account** 可复用本机已登录 Codex，无需
重复登录。Disconnect 只解除当前 Matrix 账号与本机 Codex 的关联，不退出日常
Codex。也可使用独立 Codex 登录；取消、打开登录链接、退出独立账号对应这一模式。
从真实模型目录选择模型，选择已有工作目录或准备 Agent 的私有目录，然后保存。

Codex 的“联网搜索”（Live web search）默认开启，可在此页面关闭；该选项属于
当前 Agent，并在它的多个 Room 间共用。保存后停止并重新启动服务，使修改生效。
开启后使用实时网页搜索并提供来源；本地命令的网络权限仍独立受控。

Claude Code 请先在本机完成登录，再连接本地账号；可选择 Sonnet、Opus、Haiku
本地别名和支持的推理选项。Octos 请先配置本机 profile，再选择具体 profile；
即使多个 profile 使用同一模型也分别保存，推理选项沿用 Octos profile 设置。
凭据保留在原应用中，Hagency 只保存当前主人的连接选择。断开连接不会退出原应用。
切换运行时前先停止服务，保存配置后重新启动。

Claude／Octos 的原生工具仍受 Agent、Room 和请求者三层权限限制；任一层拒绝
都会阻止操作，需要主人审批的操作显示在现有待审批列表。当前 Room 文件工具开关
属于 Codex 的 Hagency 文件桥接，未给 Claude／Octos 注册这一桥接。

模型／工作目录属于该 Agent，在它的多个 Room 之间共用；对话、Room 配额和审批
各自隔离。服务器管理员不审批本地资源选择。

在“限制”中选择整个 Agent、某个 Room 或 Room 中的某个成员。默认不限制 Token；
已设置的各层额度同时生效，支持累计、每天（UTC）、每月（UTC）。0 额度停止该范围
使用；已有 Unset 也表示不限制。请求可 Allow、Deny 或 Ask owner；
工具策略可拒绝高风险、逐个精确工具询问或限制精确工具／目录。保存保留已消耗
量和未知预留；有 hold 不表示实测消耗为零。

## 启动与聊天

在 **服务运行** 选择活动主人私聊或讨论 Room，等待已保存的本地运行时
配置及账号校验完成。默认使用估算记账，填写正数单次预留量，在“模型与资源”中保存模型支持的
推理档位。实际用量可能超出估算；未记录用量按0处理并自动释放预留额度，执行结果不明的旧请求不会自动重跑。受限 host files 默认关闭，仅提供 list／read／create。要运行命令、修改已有文件或使用 MCP，请在执行设备的 **模型与资源 → 启用 Codex 编程执行** 中开启并保存，然后重启 Agent 服务。

从 Agent 详情进入时默认选择主人私聊，不需要再次选择 Room。服务页直接显示当前聊天；切换其他房间、查看该房间的 Agent 和管理加入状态在折叠的“高级房间设置”中。从项目 Room 进入则保留对应 Room，不会切回私聊。

**开始处理消息** 只启动当前范围，**停止处理消息** 只停止它；另一个运行中 Room
可以继续工作。主人私聊无需 @，普通顶层私聊消息共用 Room 对话上下文，显式
讨论串仍隔离。协作 Room 默认每条消息都需要 Matrix 真正的 Agent @ 提及，包括同一讨论串中的追问。
在 Agent 详情 → 高级房间设置中选择协作 Room，在“Room 回复方式”开启“同一话题内自动回复”，
该话题被 @ 激活后，所有成员的后续消息无需再 @。新话题仍需 @；设置只影响该 Agent 在所选 Room 的服务。
实际模型 turn 开始后，傀儡可在对应用户消息上添加 👀 处理反应。

Codex 编程模式使用此 Agent 的工作目录；同一个 Agent 的 Room 共用该目录。请在额度与策略中为 Agent、Room 和请求者三层分别设置高风险工具审批或精确允许规则；任一层拒绝都会阻止该操作。它不提供 Room 之间的操作系统级文件保密隔离。

**MCP 服务器（JSON）** 使用明确配置，例如 `{"docs":{"transport":"http","url":"https://example.test/mcp"}}`。本地服务器使用 `{"local":{"transport":"stdio","command":"/完整真实路径/server","args":[]}}`。只启动这里保存的服务器，不导入个人 Codex 服务器或 hooks。此设置只在执行设备上管理，不通过旧版远程模型配置传送。

待审批请求／工具显示在 服务运行标签。批准针对所显示的精确请求或工具，不是服务器
无限资源授权。

### 主人加密私聊

当前运行链已实现主人私聊中的加密任务、Codex 编程执行、加密回复和文件收发。
使用该路径时：

1. 在指定执行设备配置本地 Codex 和 Agent 工作目录。任务需要命令或文件修改时，
   启用 Codex 编程执行，并保存适用的预算和工具策略。
2. 在 **Agent encryption identity** 中按需初始化身份。通过已验证的主人设备核对
   Agent 的完整 master key 和设备指纹，再使用 **Verify exact Agent identity**。
   缺少私人签名密钥时按恢复提示处理；缓存的 verified 标签不足以放行。
3. 使用主人私聊。新建主人私聊默认加密；已有明文私聊需要显式升级。检查该对话
   的实际加密状态，再显式启动该范围的服务。
4. 在该加密私聊发送任务或附件。命令和文件仍受现有审批约束；检查实际回复和
   文件投递结果。

共享 Project 请求者信任、跨主机密钥恢复尚未交付。[现有端到端验收](../../../dev-vibe/agent-encrypted-codex-validation-2026-10-27.zh.md)
证明了 macOS 上的主人链路和同主机服务重开，使用的是隔离 OAuth 测试桥接。
人工桌面操作、生产 OAuth 与其他平台的完整资格验证仍未完成；Pasion 浏览器
授权仍是支持的登录路径。普通 Matrix 加密聊天功能独立保留。

### 重启恢复（待最终验收）

重启会恢复已保存的运行意图。规则是只恢复曾明确 Start、且没有显式 Stop 的 Room，重新核验当前主人、设备、租约、资源和完整账本后使用保存的预留量、effort 与 host files 选择。**停止处理消息将该范围排除在重启恢复之外**；正常关闭程序不是同一个指令。Agent active 或保存模型不能自动启动全部 Room。资源缺失、未知费用、失去成员权限或指定其他设备均应阻断，不自动重推理或 takeover。

Agent 不限额不会绕过 Room／成员的有限额度或显式请求拒绝。已送出的策略暂停回复不算正常推理成功。[运行恢复记录](../../hagency-client/docs/design/2026-10-08-agent-runtime-recovery.zh-CN.md) 记录普通/快速重启、显式 Stop 排除及尚未覆盖的实机分支。

## 失败与恢复

| 状态 | 处理方式 |
| --- | --- |
| Loading／暂不可用 | 检查连接后刷新；旧账号／旧页面的晚响应不能覆盖当前页面。 |
| Agent creating／主人私聊 pending | 等待或重试原请求，不另建身份／Room。 |
| 指向另一执行设备 | 使用那台设备，或显式切到本机并满足账本恢复检查。 |
| 模型／目录／账号缺失 | 在“模型与资源”中连接账号并保存模型；私有工作目录自动生成。 |
| 配额未设／拒绝／等待审批 | 修改正确层级或审阅精确待审批请求。 |
| 消耗／副作用未知，或 ledger recovery required | 保留 hold，恢复同一主人的完整状态，不清余额、不重跑模型。 |
| 主人资格丢失／傀儡被移除 | 恢复真实权限、等待确认退群，再显式重新绑定/采纳；旧 dispatch 继续隔离。 |
| 普通服务暂停 | 解除对应暂停后，在允许时显式 resume。 |
| 主人加密私聊不可用 | 检查 Agent 精确身份、已验证主人设备、对话加密状态和当前执行设备授权，通过后再启动。 |
| 共享 Project 加密请求 | 请求者信任尚未交付；保持拒绝，主人的任务可使用已验证的主人私聊路径。 |
| 跨主机加密密钥恢复 | 尚未交付；同主机服务重开证据不能授权新主机或重建其密钥。 |

## 其他保留页面

### Windows 启动与系统托盘

在 **设置 → 通用 → 启动与系统托盘** 中，可设置登录 Windows 时自动启动、启动后隐藏、
最小化到托盘，以及关闭主窗口后继续运行。开机启动和启动后隐藏默认关闭，最小化和关闭
到托盘默认开启。这些设置保存在本机，对所有账号生效。
点击 Hagency 托盘图标恢复窗口，右键可打开**设置**或**退出 Hagency**；再次启动相同
数据目录的应用会唤回已有窗口。Windows 可能将图标放在隐藏图标区域，可展开后固定显示。
设置页也提供 Windows 启动应用设置入口，系统可能在该处另行禁用开机启动。
目前托盘功能适用于 Windows 独立桌面应用。

**Chats** 保留搜索、附件、Room 信息、成员与邀请；**Contacts** 可发起普通聊天。
**Mini apps** 保留非 Palpo 内置应用、App Hub、本地导入、权限审阅和受限运行。
**More** 中有 Discover、Explore rooms、Moments、File Transfer、Article editor、
Browser、Settings。

设置分为 Account、Preferences、Privacy、About。通用外部 Agent Access／
BotFather 控件与原生 Hagency Agent 设置不同，不是运行所必需。当前部分管理文字
仍为英文，因此本文使用源码中的英文控件名以便定位。


成员资格丢失或 Agent 被移除后，当前 server 会撤销绑定并确认退群；恢复真实权限后
需显式重新绑定/采纳。普通服务暂停才使用 resume，不自动重放旧 dispatch。
构建与本地连接排障见[开发指南](local-development.zh-CN.md)。

### 审批状态与提醒

服务页仅展示选中 Agent 和房间绑定的未过期审批。打开服务页时每三秒自动刷新，新请求以应用内通知提示；详情显示工具、房间、请求者、工作目录、风险、过期倒计时和准确参数。刷新失败会禁用审批操作。决定提交后不能重复提交同一请求；收到提交确认不代表工具执行成功，实际结果应查看房间回复和运行状态。提交结果不明时不要重发。当前不提供系统后台通知或持久化审批历史。
