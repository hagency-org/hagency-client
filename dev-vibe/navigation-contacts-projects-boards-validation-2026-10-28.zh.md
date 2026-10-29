# Contacts、Group Chats 与 Projects 导航优化验收

日期：2026-10-28。Client 与 Server 在原工作树修改，保留本轮之前已有的列管理、标签和快捷建卡改动；未提交或推送。此 checkout 未提供 task-writer，文档不是 canonical task 状态。

## 行为与边界

- 私聊统一进入 Contacts，联系人右侧承载真实 RoomScreen；同一人的多个私聊房间在右侧会话列表分别选择，保留各房间的时间线、通知、未读、线程和历史跳转。联系人默认未读优先，也提供未读筛选。私聊邀请同样属于 Contacts，以邀请卡片接受/拒绝并参与提醒。
- 从 Contacts 离开时释放时间线占用，重新进入时恢复；处理旧 Dock 私聊页的时间线所有权转移，避免旧布局抢占而出现空聊天窗口。异步联系人请求绑定账号、来源和 session epoch，切换账号后丢弃旧结果。
- Chats 的枚举、动作、导航 ID、欢迎页模块和组件改成 Group Chats；该列表排除私聊和所有登记到项目的房间，包括归档项目房间。项目分类未确认时不把可能属于项目的房间临时暴露为群聊。
- 新建 Project 固定创建新 Space；移除现有 Space 选择器。房间创建仍保留所需连接功能。
- 项目支持归档、恢复和“显示归档项目”；归档保留任务、看板与房间数据，停止看板写入。远端归档即使未增加看板事件序号，也会通过 readOnly 状态同步到当前客户端。
- Overview 为首个页面，展示看板、成员、任务、完成、逾期统计和最近十条项目活动。活动展示当前成员名，不读取聊天消息。
- Boards 支持项目内多个命名看板，默认 Main Board。旧看板保持原 scope；新增看板拥有独立 scope，各自隔离任务、列、幂等回执和增量游标。客户端仍用真实父 Project 进行路由授权。
- 左侧项目展开按看板在前、房间在后显示，图标区分；Rooms 不再作为项目详情主标签。
- Access、项目归档依赖当前实际策略管理权限；建房间读取 m.space.child 权限；邀请读取实际邀请权限；房间 Access 读取该房间权限，不能借用项目管理员身份。服务端再做当前成员身份和新鲜管理员状态校验。
- Members 去掉介绍性噪音，刷新、邀请改图标按钮，Project 邀请文案及提示一致。管理身份显示 badge，已验证归属的 Agent 紧跟所属成员并缩进；没有可靠归属的 Agent 保留独立行，不猜测 MXID。系统账号单独标识，Agent 元数据失败会明确提示。

## 数据与协议

Server project-board-v5 迁移增加 Project.archived、board owner_project_id、名称与创建幂等键。新增目录、Overview、归档和显式看板 scope 路由。迁移保留既有 tasks 外键及 board scope，不搬迁旧任务；跨 Project 的 scope 无法解析。新增看板创建使用项目锁串行处理，避免重复创建和 share/update 锁升级死锁。

OpenAPI 同步目录、生命周期、显式 scope 路由和只读标记。能力检查与本地启动 readiness 一并升级 v5。

## 自动验证

- Client hagency-app 全量库测试：667 passed，0 failed，2 个原有 ignored。
- Server 使用独立临时 PostgreSQL 数据库运行完整 agent-service 测试：131 passed，0 failed。包括多看板隔离、重开持久化、归档写入拒绝、恢复原数据、过期权限和跨项目拒绝；不使用应用数据库跑测试。
- Client、Server 自身严格 Clippy；Client 命令使用 --no-deps，第三方 article-makepad 的既有 lint 不作为本包新增错误。原始含依赖 Clippy 失败日志保留，不记为通过。
- OpenAPI：127 个 owner API 操作、4 个 Appservice 操作验证通过，验证器 34 项自测通过；本地启动器 16 项自测通过。
- 新增/扩展原生组件测试覆盖创建看板跨实例幂等键不复用、失败保持原意图、未读联系人排序和筛选、桌面分栏与手机返回、旧时间线所有权转移、项目创建选择器隐藏、权限入口独立性、成员 Agent 归属排序、归档只读控件（包括缓存列头随归档/恢复更新）与多看板路由。
- 原生样式审计 301 文件、0 违规；翻译 2632 条、0 缺失；工作区边界 30 包；git diff --check 通过。

## 实际原生验收

使用现有本地虚构 A/B 账号和测试项目，不调用 Agent/模型，不使用生产账号，不发送额外聊天消息。

已验证私聊在 Contacts 右侧展示、Contacts 与 Group Chats 往返、旧 Dock 会话接管、多个看板切换数据隔离、项目归档与恢复。实际发现并修正了原生 DropDown/PortalList 的 visible 不生效问题，使用 View 包装隐藏；修正归档被误认成未确认写入、私聊新建分支仍打开 Dock、旧 Dock 抢占 timeline、输入框每次事件重置 disabled 样式等问题。

B 在 Contacts 看到私聊邀请提醒和未读筛选，点击联系人后右侧打开邀请卡，接受后原位进入聊天；随后切换 Group Chats，私聊和项目房间均不在其中。B 的项目详情和菜单均无 Access/Archive，但实际 Space 邀请权限允许邀请，因此成员页和菜单保留邀请入口；不是按“普通成员”标签一刀切隐藏。A 显示管理按钮。B 的 Overview 显示 1 看板、2 已加入成员、7 任务及最近活动，成员页明确显示 Administrator 和 Invited · Service。

重启后原生新建 Navigation QA 看板、自动选中新看板、显示独立空列、侧栏返回 Main Board 后原 4 任务仍在。项目树看板使用方格图标在前，讨论房间使用聊天图标在后；新项目表单只有名称，不存在旧 Space 选项。

实际验收还复现进程计数幂等键在重启后与旧看板创建回执碰撞（board_intent_conflict），已改 UUID v4，并新增跨组件实例不复用、失败保持原请求的测试；随后真实创建成功。归档保留 4 任务，显示 Archived · read only 和恢复按钮，隐藏写入工具栏；列头缓存遗漏 readOnly 导致旧“＋”残留，已补齐缓存依赖和绘制状态切换测试。验收项目最终恢复为活动状态。

最终构建为 client-1d3df8fe-1f992a138e-730bf966，Server 为 server-617a536f-d7a2b459e1-b31011d3。双方 doctor 的源码来源、完整产物与校验和通过。A/B 均使用最终包，B 通过原子替换可执行文件并重新签名更新，仍使用各自 SDK 数据目录。最终原生复测归档后列头“＋”消失、恢复后新建入口回归、原 4 任务保留；B 重启后仍可在 Contacts 打开已接受的私聊。原始构建、测试日志与原生截图仅放在本地临时目录或 Server .run/local-dev 忽略目录，不包含在 Git 改动中。
