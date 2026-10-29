# Hagency Desktop Project 工作区与本地 Agent 整合

状态：此前已完成真实 HTTPS/Pasion 登录与基本管理页验收；当前按最新反馈重整栏目目录和 Room 信息列，最终记录见功能导航复核文档。历史测试记录保持原时点含义。

## 产品边界

本轮改造目录为 `chrislearn/hagency-desktop`，复用 `chrislearn/hagency-client` 的 owner 引擎，连接 `chrislearn/hagency-server` 的新 appservice 接口。Palpo 默认功能不改动。

Desktop 是 Matrix 聊天客户端，也是本地 Agent 创建者的管理入口。Project 是 Hagency 的业务对象，绑定一个 Matrix Space；Space 本身不自动成为 Project。聊天是 Project 已登记的 Room，当前账号必须实际加入 Room 才显示可进入的聊天。Project 成员与 Room 成员分别管理。

左侧常驻全局导航为 Projects、Agents、Chats、Contacts、Mini Apps 和 More，底部为当前账号菜单；创建入口收入对应栏目右侧 ⋯。Projects 内显示独立的 Project → Room 目录，Chats 内显示聊天搜索、私聊、其他聊天和邀请。点 Project 展开并查看详情，点其 Room 打开原聊天，保持 Projects 目录；选中 Room 后最右侧显示独立成员/资料信息列。其他栏目隐藏目录。目录与全局导航分列，不能再把 Project 或 Search chats 放在 More 下面。

主导航每项配对应图标；标题右侧折叠按钮可将全局栏缩至64px，保留图标与账号头像，栏目目录保持独立。展开恢复文字和创建按钮；折叠偏好沿用当前账号的AppPreferences保存。Project目录条目右侧 ⋯ 提供概览、讨论组、成员、邀请成员与打开Space。邀请只作用于绑定Space，按当前账号/服务器/epoch和Space权限复核；不会隐式加入讨论Room。最终实机验收记录与截图见功能导航复核文档。

右侧显示当前聊天或原生管理页面。Project 列表、Project 详情、新建 Project 分开呈现。新建默认创建 Space，也可选择当前账号有权管理的已加入 Space。Room 创建与绑定从选中的 Project 详情进入，不将 Room 和 Project 混进一个创建类型列表。

Agents 入口只展示当前用户拥有的 Agent。新建 Agent 只填写名称，创建服务器范围的 Appservice 傀儡身份；主人永久是创建者，无转让功能，不兼容 Fleet 旧结构。创建后设置绑定指定设备的执行实例，再从 Agent 设置或 Project Room 的邀请入口加入允许的 Room。默认联系人是独立主人私聊，不需要虚构 Project。本地配置包括 Agent、Room、requester 的 token 配额、请求处理策略、危险工具拒绝或逐次确认、精确工具和目录许可。Codex 是首个运行 provider，运行必须显式启动，可以停止；配额与工具策略没有服务器管理员审批。最新契约见[身份与执行实例实施细则](2026-10-07-agent-identity-execution-instance.zh-CN.md)。

## 原生复用架构

界面由 Rust/Makepad widgets 实现，聊天复用 Matrix SDK 的同步、时间线、加密与多账号生命周期。Project 和 Agent 使用新的 owner API，不使用已经退休的 Agent Operations/Fleet 协议。

本地管理复用 client 的 Console/OwnerHost 业务边界和本地 Agent 引擎，通过封闭的 typed native facade 调用。UI 不接触 access token、owner cookie 或任意 URL 路由。浏览器用于 Pasion 登录授权与回调，管理页面本身不使用 WebView。

Desktop 采用一次 SDK OAuth/Pasion 登录，同时建立聊天会话和 Hagency 管理授权。Matrix SDK 是 OAuth code 交换、refresh token、持久化与退出的唯一管理者；NativeOwner 通过 Rust 内部 token source 读取当前 SDK access token，再调用既有 Hagency session/identity/device 接口。管理界面没有第二次 Pasion 授权入口。管理授权必须匹配当前 SDK 服务器、MXID、issuer、subject 和 OAuth clientId；历史账号元数据不作为授权依据。

SDK token source 在每次读取前后检查账号 epoch、服务器、MXID 和关停状态，并通过有超时的 SDK whoami 触发必要的 OAuth 刷新。NativeOwner 不持有 SDK refresh token，不自行刷新、持久化或撤销共享 OAuth token；退出管理仅清理自己取得的 Hagency session/device，聊天 OAuth 退出交给 SDK。后台任务和晚到响应按账号会话隔离；切换账号停止旧账号任务后再打开新账号范围。

Desktop 数据目录独立于 Rinx，可用绝对路径 `HAGENCY_DESKTOP_DATA_DIR` 指定测试目录。owner profile 继续按服务器、issuer、subject、MXID 完整身份隔离。

## 依赖适配

Desktop 的 Matrix SDK git revision 使用 rusqlite 0.40，而本地 Agent 引擎保持 rusqlite 0.37；Cargo 不允许链接两份 SQLite C 库。Desktop 根 manifest 将 rusqlite 0.37 patch 到附带 MIT 许可证的 vendor 源码，只将其 libsqlite3-sys 提升至 0.38.2，与 Matrix SDK 统一原生链接。client 独立 workspace 依赖保持不变。验证包含真实 SQLite 建库和读写。

## 必须复核的状态转换

1. 未登录显示 Matrix 登录；登录后账号菜单显示当前身份，可以设置、切换或增加账号、退出。
2. 一次 Pasion 登录后自动建立 owner 授权；管理授权的有限本地 cookie 到期时自动以当前 SDK 身份恢复，不重新打开 Pasion。SDK 会话失效时回到统一登录流程，授权成其他账号不能成为当前账号的 owner。
3. Project 来源是服务器登记结果；普通 Space 不自动出现在 Project 树。
4. 展开 Project 不隐式加入 Room，点击 Room 才打开时间线。
5. 新建 Space 与服务器登记分阶段处理；结果不确定保留原 commandId，不能换 ID 再创建。
6. Agent 新建失败不制造临时假身份；配额与请求策略读写对应选中的 Room 绑定。
7. 切换账号停止旧运行、取消旧读取，并拒绝晚到 UI 结果污染新账号。
8. Codex 凭据保持在本机；测试不运行付费模型。
9. 加密聊天沿用 SDK；加密 Agent 执行遵循服务端当前支持范围。

## 验证与最终复核

### 已通过的验证

- Desktop `cargo build --offline --bin rinx` 和 `cargo check --offline --lib`。
- Desktop `cargo clippy --offline --lib`，现有代码仍有警告；严格 Clippy 会在原有 `build.rs` 的 `collapsible_if` 处失败，没有将其写成零警告。
- Desktop 22 个不同的相关单元回归：账号会话、历史账号、应用会话状态、恢复身份、响应式布局、Project 树、原生管理 UI。先运行 20 项，最终交互修正后重跑管理 UI 的 6 项，其中新增 2 项，不重复计数。包含真实 Makepad 脚本注册和管理组件实例化，未出现脚本错误。
- 上述测试包含 5 个 Project 树用例和 6 个管理 UI 用例。覆盖登记关系撤销、未加入 Room、失效记录、独立邀请、搜索与折叠、跨账号晚响应、策略保存的精确绑定及 requester、创建结果不确定时的原命令重试、成功后回列表及原详情的状态转换。
- client 原生 facade 2 个单元测试，真实 OAuth HTTP 回归 1 个，创建日志相关回归 5 个；client library 与 console 严格 Clippy 通过。
- OAuth 回归验证同账号授权、不同 Matrix 账号在落盘及设备注册前拒绝、回调重放拒绝、关停及重启后旧管理授权不可复用。该测试使用本地 HTTP fixture，不能替代真实桌面浏览器验收。
- vendor SQLite 上游 166 个单元测试通过。两个 rusqlite Rust API 共用一份 `libsqlite3-sys`，未改变独立 client 的依赖。
- Desktop/client `git diff --check`、macOS runner `bash -n`。
- 最终构建通过 macOS 开发 bundle 启动，实际原生进程存活；`desktop-final.log` 未出现 Makepad 脚本错误。此为启动验证，不是布局、点击或正常退出验收。

没有运行付费 Codex 模型，没有修改 Palpo 默认功能，也没有执行账号转让或旧 Fleet 数据迁移。

### 复审范围与修正

复审包括全部本轮 Desktop 改动、新增 Project 树、原生管理 UI、后台账号服务，以及 client 新增原生 facade 与它涉及的授权、创建日志、OwnerHost 入口。client 先前已经存在的工作区改动予以保留，不将其当作本轮重新实现的功能。

| 范围 | 实际检查及修正 |
| --- | --- |
| 单栏导航与 Dock | 移除重复图标栏和全局搜索栏，初始 Projects 页主动激活，管理页面保持左侧树，关闭后回到最近聊天 |
| Matrix 同步与 Project 树 | 隐藏同步消费者仍初始化；只将有效服务器登记与当前 SDK 已加入 Room 交集放入 Project；邀请保留原有接受流程；登记变化立即刷新，定期刷新有 30 秒上限 |
| 用户菜单 | 当前头像、名称、MXID 放到底部；复用已有账号菜单的设置、切换、增加账号与退出流程，不把登录按钮放在已登录导航中 |
| Makepad 注册 | 调整组件注册顺序；启动时发现 DropDown/TextInput 没有 `visible` 字段，改由外层 View 控制显隐，并加入脚本实例化回归 |
| 新建 Project/Room | 默认新建 Space；已有 Space 分页选择；Room 从选定 Project 详情进入；未知结果保留原输入和 commandId，查询确实不存在才允许相同命令重发；绑定成功回列表或原 Project 详情并只读确认 |
| Agent 与策略 | 创建及绑定使用稳定幂等键，成功后自动回列表读取确认状态，不重发创建请求；策略 revision 精确绑定 Agent、Room binding 与 requester，避免更换 requester 后把旧 revision 保存到新目标 |
| 账号隔离 | 每次请求固定 SDK 服务器、MXID 与会话 epoch，await 前后均检查；切换清理 UI 选择与结果，先停止旧 owner 再关闭旧聊天会话 |
| SDK 授权 | 每次共享 token 读取都执行实际 SDK whoami，并检查 issuer、MXID、device、clientId 与 epoch；8 秒超时；401/403 或身份不匹配拒绝授权，临时离线允许恢复后重试 |
| 原生授权入口 | typed Command 封闭路由；SDK 模式拒绝独立 BeginLogin 和旧 callback；有限 cookie 过期后自动以 SDK 身份重建，SDK token 不写入 owner 文件 |
| 停机 | 退出账号在可能很慢的 Matrix logout 前停止 owner；程序退出给关停 15 秒；未正常结束的执行仍保留账本记录 |
| 包与依赖 | Hagency 名称、bundle id 与数据目录独立；默认构建关闭旧 Agent Operations；SQLite 适配保留许可证和上游测试；本地测试 profile/log 放在忽略的 `.run/` |

### 已知边界与剩余验收

1. 二次授权已由 SDK 标准 OAuth 登录和 NativeOwner 共享 token source 替代；下节记录本轮实际验证。现仅允许通过服务识别的 Hagency server，认证元数据不可用时不得静默降级成密码或普通 Matrix 登录。Hagency 管理与聊天共用 SDK OAuth，会话恢复同样检查 Hagency 标识和 Pasion issuer。
2. Project/Room Matrix 创建有持久化命令恢复；本轮 Agent 创建和绑定也已补齐 owner 身份隔离的持久原命令与 Desktop 恢复页面。先查询 server 原命令，明确404后仅以原key/原payload重试；未知结果不能盲目换key新建。详见[Agent完成核对](2026-10-07-desktop-agent-completion-audit.zh-CN.md)。
3. 原生页面提供 Codex 登录、凭据定位、模型与工作目录、运行启停、配额、请求策略和逐次工具确认；实际付费执行未测试。精确风险确认沿用 client 引擎边界。
4. 普通加密聊天沿用 Matrix SDK；本轮没有增加加密 Agent 执行能力，也没有改变 Space/Room 成员关系。
5. 协议测试采用真实 SDK 与本地 HTTP fixture；Desktop 全局 UI/同步生命周期下的网络中断恢复仍需实际窗口验收，不能将 fixture 等同于真实服务器、浏览器与窗口的完整验收。
6. 真实窗口验收尚未完成：需检查登录后 Project 二层树、底部账号菜单、Projects/Agents 列表与新建表单、已有 Space 绑定、创建后即时出现、账号切换与真实 Pasion 回调。首次复核时 Mac 锁屏；用户随后解锁并允许测试应用读取可移动宗卷，实际验收继续进行。

## 二次授权修正：2026-10-07 续审

本节替代前一版的双阶段授权限制。实现使用 Palpo/Pasion 已有 OAuth 与 Hagency 管理接口，不改动 Palpo 源码或默认功能。

OAuth 的 Matrix API scope、唯一 device scope 与 token 刷新行为依据 [Matrix Client-Server API 规范](https://spec.matrix.org/v1.19/client-server-api/)；SDK 对接以本项目固定的源码版本为准。

- SDK 标准 DCR、PKCE S256、loopback callback 和 `finish_login` 建立 Matrix OAuth 会话。
- 会话文件保存 OAuth clientId、SDK UserSession、device 和对应数据库位置；SDK 刷新后原子写入新 token，Unix 文件权限 0600，账号目录 0700，Debug 不输出 token。
- 管理后台自动从 SDK 授权。普通管理页面中的再次登录只回到同一个 SDK 登录入口，不能开启第二条独立 OAuth 链。
- SDK 模式在原生命令和浏览器 callback 两处拒绝独立 BeginLogin；已有独立 hagency-client 的 Pasion 模式保持原生命周期。
- SDK 恢复或重登后，owner 重新验证完整身份，沿用该身份的本地账本和策略；不会凭历史账号直接取得授权。
- HTTPS 使用 SDK 原生 OAuth 撤销；SDK 的撤销库强制 HTTPS，因此本地 HTTP 开发服务器由 Desktop 登录层按 RFC 7009 撤销当前 refresh token（没有 refresh token 才撤 access token）。这一回退同样只允许经过校验的 loopback 地址，NativeOwner 不参与 OAuth 撤销。
- 明文 HTTP 仅允许 loopback 开发地址，OAuth metadata 中的 token、registration、revocation、authorization endpoint 也检查该限制。

本轮已通过验证：

- Desktop OAuth/登录模块 14 项、历史账号 5 项。真实 SDK + HTTP fixture 验证 DCR、PKCE S256、loopback 回调、一次 code 交换后聊天 whoami 和 Projects 管理均可用、SDK token 旋转及同步落盘、原生管理续期读取新 token，以及原生管理关停不撤销聊天 OAuth。
- 错账号、错设备的重新登录拒绝本次候选授权并只撤销新 grant；原有账号/设备保持可恢复。OAuth clientId 非法的文件不降级为 Matrix password session；交接阶段的失效 epoch 拒绝晚到授权。
- Desktop 管理 UI 6 项、账号相关筛选 22 项通过；这些筛选与上述测试有重叠，不累加成总数。管理组件的真实 Makepad 脚本注册/实例化通过。
- client 授权模块 20 项（包含新增 7 项 SDK 共享授权 HTTP/竞态回归）、原生 facade 2 项通过；client library/tests 严格 Clippy 通过。覆盖有限 cookie 的自动更新、共享 token 不落盘/不撤销、SDK 模式拒绝独立登录、账号晚响应、持锁等待时关停、断网恢复。
- 临时离线先使旧 Hagency device 失效，返回 503；NativeOwner 不永久退出，SDK 恢复后以同一完整身份自动重新取得 Hagency grant。没有将旧设备的可用性延长到未经核验的离线时段。
- 登录候选提交、SDK 同步刷新保存与账号 epoch 推进共用 SESSION_IO 边界；异步 TokensRefreshed 不再重复写盘，避免旧刷新结果覆盖新 refresh token。候选提交之前完成 callback 安装，安装失败不替换已有 token 文件。
- 最终 Desktop binary 构建、diff 检查、macOS runner 语法检查通过；新 bundle 的构建输入匹配最终 binary。新测试进程 PID 13973 已启动，`desktop-unified-login.log` 未见 panic/Makepad script error。这只是启动证据，不能证明锁屏后的原生交互。未测试付费模型。
- 三个实施/复审代理及父代理完成本轮代码复审；先前发现的共享 token 撤销、断网恢复、晚写、错误登录和交接清理问题均已修正。失败 OAuth 撤销采用有限超时的 best-effort；远端不可达时不能保证立即撤销，但本地不会采用被拒候选会话。

固定依赖源码复核：Pasion 的 Native DCR 接受不带端口的 `http://127.0.0.1/` 注册，授权时允许随机 loopback 端口；token 交换重复完整授权 redirect URI。SDK 序列化自动注册 authorization_code 与 refresh_token，两种 grant 已由 fixture 请求断言验证。public client 的 none 认证、PKCE S256 和 Matrix scope 别名均与固定 Palpo/Pasion 版本匹配。

最初部署验收受阻：当时本机 8089 的认证元数据接口无有效响应，Docker 引擎中的服务容器无法正常退出/重启。已修正测试部署配置的容器监听端口、端口映射和健康检查，使容器内公共 loopback 地址与监听地址一致；未改 Palpo 源码或默认功能。重新启动 Docker Desktop 会短暂影响另外两个 PostgreSQL 容器，已提出确认，未获答复前不执行。原数据库和 server-data 卷保留。

首次尝试时 Mac 锁屏，桌面工具无法操作原生窗口。用户随后解锁，后续真实验收与修正记录见下节；独立 HTTP fixture 与源码复核不能替代完整窗口验收。

## 解锁后的实际验收与修正

- Docker 引擎已恢复响应，只重新启动原测试部署的 server；另外两个 PostgreSQL 容器与数据卷保持运行，没有重启引擎。监听端口改为 8089 后，原 AS 回调地址仍为 8088，安全校验正确拒绝了启动。维护前核对原登记的 ID、token、sender、namespace 与启用状态，仅同步更新本部署的私有 AS 文件及数据库登记 URL 到 8089，保留凭据和身份，不放宽启动校验、不修改 Palpo 源码。
- 父代理实际确认 `/readyz` 与 Matrix `auth_metadata` 均返回 200，issuer 为 `http://127.0.0.1:8089/_pasion/` 且包含 PKCE S256。
- 真实窗口显示 OAuth 登录入口，但点击没有动作。原因是 `show_methods` 接受 OAuth，而 `start_sso` 仍只接受旧 SSO。现统一使用支持 OAuth 或 SSO 的入口判定；新增 OAuth-only 与注册能力两个回归通过。登录标题和单次授权说明纳入中英文目录，既有 i18n 三项回归通过。
- 更新后的调试 bundle 在读取 `/Volumes/Data` 中的测试配置时等待 macOS 可移动宗卷访问许可。系统日志确认原调试签名与重编后的签名不匹配，导致系统再次要求文件访问确认。这是操作系统的文件权限，与 Pasion/OAuth 的第二次登录无关。桌面工具安全限制禁止操作系统通知窗口，用户已手动允许，原生应用正常完成启动并显示中文登录页；未修改 TCC 数据库或放宽系统保护。
- 该阶段的真实登录当时尚未完成；后续已完成 HTTPS/Pasion 单次登录，具体结果见下节。

## Hagency 服务器准入与本地 HTTPS：2026-10-07

本节落实新约束：Desktop 和独立 client 暂不支持普通 Matrix 服务器。服务器必须提供 Hagency API 和集成的 Pasion 登录；不能先让用户登录聊天，再发现 Projects/Agents 管理不可用。登录页不再默认 matrix.org，空地址必须提示填写 Hagency 服务器。普通 Matrix 的历史密码会话不激活，要求重新通过 Hagency/Pasion 登录；磁盘上的原账号数据不删除。

### 公开服务识别契约

复用无凭据的 `GET /api/hagency/v1/discovery`，增添产品、软件版本和能力字段；它不颁发 cookie、session 或 device 权限。接口仍校验公共 Host，不接受浏览器 Origin/Cookie 或 X-Forwarded-Host。接口说明和 OpenAPI 位于 server 仓库的 `docs/SERVICE_DISCOVERY.md` 及 `crates/agent-service/openapi/hagency-v1.openapi.json`。

客户端共用 `hagency::server_admission`，登录发现、直接 OAuth 登录、SDK 会话恢复、独立 client 登录和原生 owner 授权都在使用凭据前检查：

| 字段 | 验证规则 |
| --- | --- |
| product | 必须为 `hagency-server`，不能仅凭 serviceMxid 存在识别 |
| version | 必须为有效且有长度限制的软件版本标识；不代替协议版本 |
| protocolVersion | 当前仅支持整数 1 |
| capabilities | 必须包含 pasion-oauth 和 owner-agent-appservice-v1 |
| homeserver | 必须与用户选定的规范化根地址完全一致 |
| issuer | 必须为该地址的 /_pasion/，SDK OAuth metadata 同样匹配 |

请求不携带凭据、不使用环境代理、不跟随重定向，响应大小限制 16 KiB，总超时 15 秒。普通 Matrix、缺能力、版本不兼容、错误 metadata 分别给出明确中英文提示；网络或服务故障表示暂不可用，允许重试，不能误认为用户已授权。TLS 保留证书和主机名校验，HTTP 仅用于精确 localhost 或 loopback IP 的开发入口，hagency.local 使用 HTTPS。

### HTTPS 测试部署与验证

- 新地址为 `https://hagency.local/`，Matrix server name 为 hagency.local，测试账号为 `@chris:hagency.local`。旧 localhost:8089 环境的永久身份、数据库和数据卷不原地改写；新开发域使用独立 Compose project、数据库和 server-data 卷。
- 宿主 Caddy 使用 internal CA，监听 IPv4/IPv6 loopback 的 443，代理到 127.0.0.1:8090。保留原 Host 并移除它自动添加的 X-Forwarded-Host，以符合 native API 入口约束，不放宽 server 校验。CA 私钥不挂载给服务器；容器只导入公开 root.crt 到正常系统信任。
- macOS hosts 同时添加 127.0.0.1 和 ::1。第一次只添加 IPv4 时，.local 的 IPv6 解析等待约 5 秒并触发连接超时；补齐后实测 DNS 约 4 毫秒、HTTPS readyz 总计约 22 毫秒。
- 用户手动执行受管理员权限保护的 hosts/系统信任安装脚本；脚本检查本次 CA 文件指纹，拒绝冲突域名，不改写其他 hosts 项并保存原文件备份。Caddy 证书与主机名验证正常，未使用 -k、固定 IP 或关闭 TLS 验证来代替系统信任。
- client 的 reqwest 同时启用公共与 native 系统 roots。仅启用 webpki roots 时无法使用用户安装的本地 CA；这次已补 native roots。真实 Rust admission probe 使用正常 DNS 和系统信任，读取 HTTPS metadata 通过，约 0.14 秒。
- server readyz、Hagency discovery、Matrix auth_metadata 和容器内部 Pasion OpenID metadata 均返回 200；实际 TLS verification 为 0。发现的 issuer/homeserver/产品/协议与新部署严格一致。一次性测试账号 bootstrap 参数和密码挂载已移除，密码仅存本地私有 0600 文件。

证书配置依据 [Caddy Automatic HTTPS](https://caddyserver.com/docs/automatic-https) 和 [Caddy 本地信任命令](https://caddyserver.com/docs/command-line)。测试配置及日志放在 server 的 `.run/hagency-local-https-20261007`，不作为发布配置提交。

本轮验证：server PostgreSQL 路由 42 项、严格 Clippy、OpenAPI 契约校验通过；client 准入 4 项、授权 20 项、实际系统信任 HTTPS 1 项、严格 Clippy 与中英文提示检查通过；Desktop 登录/OAuth 14 项、i18n 3 项、Clippy（保留既有 19 条 warning）、最终 binary 构建通过。父代理复核了准入前不发送凭据、规范化地址/issuer、重定向拒绝、响应限制、普通 Matrix 拒绝、系统 CA 信任以及代理 Host 处理。

### 真实单次登录与管理布局复核

用户在原生窗口填写 https://hagency.local 后，实际浏览器进入该服务器的 Pasion。测试账号 chris 登录并完成一次 Hagency Desktop 同意授权，loopback 回调显示完成，原生窗口进入 Projects 并显示 Verified。Hagency owner 管理授权自动建立，没有再打开第二条 Pasion 登录。SDK OAuth 会话已经落盘，会话文件 0600、账号目录 0700；复核仅读取 schema 与权限，不输出 token。桌面工具不能向 Makepad 输入框输入文字，用户手动输入成功，因此未将自动化输入限制认定为应用输入缺陷。

首次登录后的真实窗口暴露了两个布局缺陷，已修正并复审：

- 窄侧栏按自己的宽度选择了 Mobile variant。现在跟随 HomeScreen 的实际布局，并在主布局变化时重选。真实单 Home 绘制回归覆盖 Automatic 桌面/移动、ForceWide、ForceNarrow、回到 Automatic；每阶段断言 Projects/Agents 的存在和 RoomsList 与全局共享同一 Widget UID，避免新增 SDK 元数据消费者。新增 1 项与既有响应式 2 项均通过。
- PortalList 会绘制范围之外的填充行，原实现每一行重复显示空状态；现仅空列表的第 0 行显示提示，其余填充行留白。PortalList 的 set_visible 无效，现由外层 View 控制列表显隐，详情/创建表单不再被旧列表挡住；返回按钮只在详情或表单出现。管理 UI 8 项通过，包含真实脚本注册、第二个 Project/Agent 卡片的分组点击及准确选择、列表显隐和返回状态。

父代理再次检查上述源代码及状态转换。最终 binary 构建通过（22.98 秒），旧进程由应用菜单正常退出，新构建已启动。该构建后续已进入实际窗口，底部显示 @chris:hagency.local，修正后的 Project 列表显示用户创建的 testproject，空状态不再重复。完整创建、账号切换与网络恢复仍需要分别验收。没有将绘制回归写成真实创建闭环通过。

### 启动与测试资料

原生开发应用：`chrislearn/hagency-desktop/target/debug/.rinx-dev/Hagency Desktop.app`。运行：`cargo run --offline --bin rinx`。测试 profile 通过绝对路径 `HAGENCY_DESKTOP_DATA_DIR` 隔离于用户默认目录，本次放在 Desktop 的 `.run/desktop-project-workspace-20261007/profile`。

原重构启动日志为该目录的 `desktop-final.log`；本次统一登录初次启动日志为 `desktop-unified-login.log`；解锁后修正 OAuth 入口并重新构建的实际窗口启动日志为 `desktop-window-login.log`。新 HTTPS 测试窗口日志为 `.run/hagency-local-https-20261007/desktop-https-login.log`，截图为该目录的 `hagency-only-sign-in.png`；隔离测试数据位于 `~/Library/Application Support/Hagency/Development/https-local-20261007`。本轮源码指纹清单共 130 个文件，另存于 `2026-10-07-desktop-code-review.sha256`，只覆盖本轮源代码与构建适配，不含账号数据、token、日志或构建产物。

## 功能入口补全：2026-10-07

用户复核发现旧导航功能未完整迁移。已将唯一左栏上移到 HomeScreen、补回私聊/联系人/Mini Apps/动态/文章/文件/探索/浏览器入口、恢复 Room 操作菜单，并补上 Project Space 与独立 Room 成员名单。完整的保留/移除矩阵、入口设计、成员权限与回归记录见 [功能保留与导航复核](2026-10-07-desktop-feature-navigation-review.zh-CN.md)。本节和该补充文件替代前文仅有 Projects/Agents 两个入口的布局说明。

用户允许外置磁盘读取后，实机仍发现侧栏占半宽、菜单需要滚动和账号第二行裁剪；因此前一构建没有通过布局验收。已修外层 280px 固定容器、菜单高度、账号子元素，并细分主题卡片。最终 Home 39 项、原生管理 10 项、i18n 3 项通过；Clippy 保留既有 19 条 warning。最终构建与实机结果见功能复核补充文档。

### 左栏栏目操作菜单修正

左侧仅Projects、Agents、Chats、Contacts、Mini Apps五个主栏目；Projects/Agents/Chats右侧 ⋯ 提供创建动作，New Project/New chat不再独立占导航行。More为七工具浮层菜单，不挤压聊天树。详见功能复核补充文档本日追加。
