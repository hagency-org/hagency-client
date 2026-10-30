# R13：应用内 Matrix 房间和事件链接改进

本批从 `361d4e34ae5be9cb530b6418d023c50fba02c1ee` 开始，在独立 managed worktree 的 `codex/quality-matrix-links` 分支实现。没有更改 SDK、依赖锁或其他质量批次工作树，也没有连接真实账号、执行 provider 推理或部署。

## 已实现的行为

- 普通 URL 入口、对话中的房间/别名/事件链接、Space 已有 URL 入口统一进入应用内 Matrix 路由。使用已锁 Ruma 类型解析 `matrix:` 和 `matrix.to`，保留原始 via 和 action。明确选择“在浏览器中打开”仍走原外部入口。
- 已识别的损坏、过长或不支持链接给出本地错误，不因失败而外开。URL 在进入 Ruma 和原用户资料兼容解析前限制为 8192 字节；长 URL 的 host 识别不分配新长字符串，覆盖 query 和 userinfo authority。
- 别名解析、房间预览、事件和 root 校验通过发起时的真实 SDK Client。未加入房间只展示已有原生确认模态；`action=join` 本身不触发加入。确认后仍使用原 resolved room ID 和 via。未知结果不会声称服务器已撤销加入。
- 普通房间、线程和 Contacts 私聊分别进入原有布局。先确定真实 `SelectedRoom`、`TimelineKind` 和输入框原上下文，再复用历史事件定位；线程回复不会保留主线 composer。
- 对密文事件使用真实 SDK Megolm 解密。先移除 SDK 兼容逻辑可能补入的未认证外层关系，线程归属只接受已认证的内层 `m.thread` 和可访问的原 root。只有外层 thread 提示而缺少内层 root 时明确失败，不猜测跳到主线。
- 从输入排队到确认、结果投递、Contacts 延期绘制和待定位事件，持续保留原 WeakClient、账号 epoch、原 SDK SessionMeta 身份及请求 Arc nonce。新意图、页面导航、取消和账号退役使旧结果失效；原聚焦 build 的请求撤销会立即 Drop 实际 pending build。打开其他加入/离开模态同样替换旧意图，旧 Confirm、Dismiss 和 Finished 不能关闭新模态。
- 错误保留原输入或 Ticket 的 epoch，使用现有 scoped popup 队列；账号切换后，队列中的旧错误不能重新显示，已显示错误会被真实 UI Signal 清除。

## 被实际绘制发现的被动启动问题

首次生命周期结果为 **6 pass / 2 fail**，失败是实际原生导航创建了 local execution owner。原失败及每轮实际调用栈保留在 `.run/quality-matrix-links/lifecycle.json` 和 `owner-startup-*.log`，没有改断言或改称通过。

实际绘制依次暴露了以下纯观察入口误用 `Service::for_current`：Room 的 AgentRuntimeNotice、Contacts badge、AgentProfileDetails、ProjectTree、RoomNavigation，以及房间信息栏的成员归属查询。`for_current` 是可能创建 owner 并恢复已启用 runtime 的工厂，不能当只读查询使用。

本批只将这些实际被动读和响应身份验证改为 `existing_current`。后台成员归属和 Contacts 查询在 UI 生产者处捕获原 Service，再交原任务，不在迟到 future 中重新读取新账号 owner。无服务时明确显示不可用、未运行或归属无法验证，不伪造空成功。基础 Matrix 搜索、媒体、信息和返回仍按原账号 Pin 可用；项目变更仍要求原 owner 授权。

明确的 `LoginSuccess` 恢复入口和显式启动、创建、绑定等操作保留原工厂。Backend API 文档说明了两者副作用，临时回溯代码已移除。

## 父审查后的根导航补充

`a699903c` 后复核确认，Delivering 阶段将普通 GoHome/Contacts 视为内部跳转，会让人类替代导航后的旧 Ticket 继续存活。内部步骤现改为携带原 Ticket 的 `EnterHome` / `EnterContacts`；所有普通导航都取消旧意图，每一轮 UI 消费和延迟绘制重新校验。

真实1280 HomeScreen 从 Settings 切回 Home 的正例又检出初始 dock 顺序错误：Navigate 先排队，首次绘制才排队的 LoadDock 随后覆盖已选 tab（`root-routing-focused-third.log`，12 pass/1 fail）。Home 现只保留最多一个原导航，等待子树绘制结束、原 LoadDock 先排队后再投递目标；普通 Room 和真实 SDK Space 均必须跨后续动作/绘制保持选中，未放松断言。

初始化中的隐式 OpenProjects 会调用 owner 工厂，新增实际 NoBootstrap 失败保留在 fifth log。原 Desktop 现在记录 permalink 初始化的来源，直到原 LoadDock 实际消费，Ticket 失效不会变成创建 owner 的权限。真实1280负例先确认原 LoadDock 已排队，再转 Settings 退休 Ticket，把延迟事件交同一原 Desktop 实例处理，确认标记消费且 CURRENT None、startup0、没有 owner 目录。显式 Home/Projects 仍沿原工厂路径；未修改 HagencyPanel 的显式 action。

## 验证证据

所有测试从本分支实际编辑路径构建。Cargo 显式使用 Data 卷共享 target；每个有 SDK 全局状态的新增测试在独立子进程、私有临时 App profile 和 loopback server 中运行，并检查子进程确实执行了一个 selector。

| 验证 | 证据与结果 |
|---|---|
| a699 原精确 App CI 特性 Clippy | `agent_chat,octosense-module,macos-sparkle`；App + mini app core、全部 targets、测试及 examples，`--no-deps -D warnings` 最终通过，29.36 秒（前次20.91秒记录保留） |
| a699 原同特性完整 App 库测试 | `final-ci-profile-app-suite-second.log`：821 pass、0 fail、2 ignored，77.39 秒；原820/0/2首轮记录保留，新增实际旧模态回调回归；保留原两项 ignore |
| 原生 UI 和 SDK 行为 | 实际确认按钮动作、未确认零 join、确认保留 via；实际 alias/event/context HTTP；真实 Megolm 内外关系；桌面1280/compact375/Contacts实际控件及 composer；原 Client 替换、取消、epoch、旧弹窗 |
| 深层零 bootstrap | 所选真实 RoomScreen 单独实际 draw，Contacts badge 原后台请求完成后再断言；CURRENT None、startup count0、native-owner 目录不存在；原第七波2/2通过 |
| a699 原无默认特性编译 | 显式移除Sparkle SDK环境，`--no-default-features --features agent_chat`，51.67秒通过 |
| 本地化与主题 | 2755 个词条，2025 个已翻译调用点、missing0；Theme literal guard 通过 |
| a699 原 Task Contract | `final-lifecycle.json`：agent-spec1.4，8/8 pass、quality1.0；0 fail/skip/uncertain/pending，含边界和7个场景；默认Cargo profile，精确CI特性另测 |

根导航补充源码验证为 `root-routing-focused-seventh.log`：**13/13通过，4.84秒**；就近既有 Home responsive **5/5**、MainDesktop persistent **5/5**；同精确CI特性的 App + mini app core all-targets 严格 Clippy最终 **15.06秒通过**。上述821项完整测试和8/8生命周期属于补充前的 a699 完整验证，**不冒称当前补充源码已跑完整套件**。父任务将在合成最终分支后统一运行完整精确CI profile与完整生命周期。

SDK 夹具复用项目已有 loopback sync/context 服务，开启真实 SDK threading 支持。主线和线程的实际 focused timeline 都必须包含原目标；错误 main/thread 归属必须拒绝。取消测试使用原 production build/watch helper 和受控 pending future 的 Drop 观察，确认原 future 确实释放；这项不冒充底层 SQLite/OS IO 的中断测量。

## 保留的边界

- 这里实现的是应用内路由和既有 NativeDeepLink action 的消费。已锁 Makepad 只发现 Android producer；macOS/Windows 操作系统 URL 发出端、冷启动投递、关联注册的实际桌面端到端仍未 qualification，本批不宣称支持。
- 事件的 512KiB 是 **SDK 已缓冲 HTTP 响应后的检查上限**，不是原网络 body 的早停或流式内存保证。
- 本地 SDK/控件夹具不代表生产 federation、TLS、真实账号 E2EE key recovery 或真实桌面 OS URL 投递已通过。
- 已发送的 join 可能在取消观察后完成；保留原服务器结果未知的说明，不伪称撤销。用户仍需服务器授予的真实权限。
- 本机只验证 macOS arm64；其他平台及 GitHub CI 必须在后续最终 PR head 上独立通过。当前分支未 push、未开 PR，等待父任务独立审查。


## 父复核后的组合源码验证

父工作树在`a30a42cd`基线上依次整合两项已审查提交，当前实现提交为`76024fe2`，
分支为`codex/quality-matrix-navigation`。25个实现/契约/报告文件的Git blob与
agent最终源码逐一相同；两份协调文档通过共同前缀验证合并，保留两边完整追加记录。
父复核确认普通导航取消原意图、内部根切换携原Ticket、首次desktop draw后再投递，
以及退休初始化不会因Ticket失效而获得启动管理runtime的权限。

- 精确CI特性`agent_chat,octosense-module,macos-sparkle`完整App：824通过、
  0失败、2原有忽略，78.17秒；这是当前组合源码结果，替代此前821项的局部基线。
- own App与miniapp core all-target严格Clippy通过（29.43秒）。
- 完整agent-spec生命周期8/8通过、0失败/跳过/不确定、质量得分1.0；
  显式核对全部27个改动路径并执行七组真实Cargo绑定，未把空worktree当边界证明。
- 全仓typos1.51.1、主题literal guard通过；2755词条、2025翻译调用点、0缺失。

本节记录本地冻结时证据；当前提交的hosted CI仍待PR采集后独立复核，不能用本地
结果或旧提交平台结果替代。系统URL投递和真实账号E2EE恢复仍未取得平台资格。
