# Hagency Client 项目质量评审与改进报告

评审日期：2026 年 10 月 29 日。源码基线：`1c53bd76a63962832c8d85d1c87823efc51b53fc`，分支 `codex/tutorial-regressions`。对象是当前 Rust/Makepad 原生桌面程序及同仓库本地 Agent 引擎，包括独立打包的 Octos 运行时；不是已退休的 CLI Web 控制台。

**总体判断：项目已具备较丰富的功能和较强的授权、账本、恢复设计，但还不宜仅凭构建成功和单元测试通过，宣称达到完整、稳定、安全的生产质量。** 最值得优先投入的是文章图片请求边界、TLS 与独立运行时的依赖更新、普通附件传输的资源限制，以及发布验收门禁。产品侧的主要缺口是审批发现、快速创建草稿保护、消息链接跳转和多媒体体验。

本报告由主评审与安全、性能、功能三个并行 agent 汇总，主评审复核关键代码和过滤函数。未修改业务代码、账号或服务配置，未启动真实 Agent 或付费模型。原有 `dev-vibe/tutorial-20261029/` 未跟踪内容保留原样。源码规模、测试数量和历史截图不被换算为功能完成率。

## 质量维度结论

| 方向 | 判断 | 主要依据与差距 |
| --- | --- | --- |
| 安全边界 | 有较强基础，存在优先修复项 | 文件私有存储、账号 epoch、审批新鲜度、更新验签较严谨；文章图片内网过滤可绕过 |
| 依赖与供应链 | 当前主要风险之一 | 主锁文件 21 条漏洞匹配，独立 Octos 锁文件 47 条；缺少持续审计门禁 |
| 性能与资源控制 | 预览/时间线较好，普通附件路径不足 | 图片、GPU、时间线有预算；普通下载/上传仍完整缓冲，资料缓存无容量上限 |
| 稳定性与恢复 | 引擎设计较深入，仍需当前版本组合验收 | 未知结果保留、停止后 join、账本和重放防护值得保留；不能用局部测试证明所有平台完整恢复 |
| 功能实现 | 核心面较丰富 | Project、Board、Room、Agent、额度和审批均有实质代码；音视频播放和 permalink 存在明确缺口 |
| 功能完成度 | 核心可用，端到端闭环尚未齐全 | 审批提醒依赖页面打开，快速建卡可丢草稿；共享加密请求者/跨主机恢复等须分别标注 |
| 数据与隐私 | 账号隔离较好，静态凭证与临时明文需改进 | 会话 token 与数据库口令同文件；分享附件副本清理依赖下次启动 |
| 测试工程 | 单元与契约门禁较强，发布资格链仍有断点 | 本轮桌面库 706 通过；真实主链资格记录仍有 placeholder，部分测试有明确忽略/跳过 |
| 跨平台与发布 | 打包和更新机制较完整，阻断规则需统一 | Windows 引擎全矩阵允许失败；发布工作流只依赖安装包 job |
| 架构与可维护性 | 包边界清晰，热点模块仍偏大 | 30 包边界检查通过；若干 UI/同步文件承载多个职责，审查和回归成本高 |
| 可观测性与支持 | 故障证据意识较强，运行指标需扩展 | 应继续保留原始失败；补附件字节额度、队列、索引滞后、审批等待等指标 |
| 文档与无障碍 | 文档存在过时能力声明；无障碍结论待实测 | 翻译调用点零缺失不等于产品全部中文化；读屏、缩放、仅键盘流程尚未在本轮验证 |

## 风险优先级

P1 表示应在下一次广泛生产发布前处理或完成明确的阻断性分流；P2 表示应进入近期迭代；P3 表示后续质量改进。优先级包含控制缺口和产品风险，并不表示所有条目都是已利用的安全漏洞。

| 编号 | 优先级 | 问题 | 证据类型 |
| --- | --- | --- | --- |
| R01 | P1 | 文章图片可绕过本机/内网地址限制 | 当前过滤函数隔离复现 |
| R02 | P1 | TLS 依赖过时，独立 Octos 安全债未纳入持续门禁 | 新鲜审计、默认依赖图、发布脚本确认 |
| R03 | P1 | 普通附件下载缺少全局并发和实际字节预算 | 静态确认，OOM 风险未实测 |
| R04 | P1 | 发布不绑定完整引擎及真实主链验收结果 | 工作流与资格文件确认，非业务漏洞 |
| R05 | P2 | token 与数据库解密口令同存明文 JSON | 当前持久化实现确认 |
| R06 | P2 | 分享解密附件留下生命周期不明确的明文副本 | 当前写入/清理路径确认 |
| R07 | P2 | 上传完整读文件，读取量可能超出先前检查 | 静态确认，内存峰值未测 |
| R08 | P2 | 下载回调持全局锁执行同步写盘和分享 | 静态锁作用域与调用链确认 |
| R09 | P2 | 用户资料缓存/完成队列无容量与 UI 批次预算 | 静态扩展性风险 |
| R10 | P2 | 大房间历史追赶阻塞其他房间索引更新 | 静态调度风险 |
| R11 | P2 | 快速建卡草稿切换 Board 时静默丢失 | 当前可达代码路径确认 |
| R12 | P2 | 审批缺少跨页面的全局待办发现能力 | 当前轮询条件确认 |
| R13 | P2 | Matrix 消息永久链接和 alias 未完成原生导航 | 当前链接分支确认 |
| R14 | P2 | 音视频时间线播放未完成 | 当前渲染实现确认 |
| R15 | P2 | 加密 Agent 能力和隐私文档与实现不一致 | 当前文档/生产路径对照 |
| R16 | P3 | 协议版本返回值错误，热点模块需逐步收敛 | 当前返回值/调用者与模块结构确认 |

## 需要优先处理的问题

### R01 文章图片的内网过滤可以绕过

位置：[remote_images.rs:7](../crates/hagency-app/apps/article-editor/native/remote_images.rs#L7)、[图片自动加载入口:1704](../crates/hagency-app/apps/article-editor/native/ui.rs#L1704)。

`allowed()` 的域名分支只检查名称含点和不以 `.local`/`.localhost` 结尾，没有审核 DNS 解析目标；IPv6 分支没有识别 IPv4 mapped 地址。审核后直接 GET，重定向也使用相同检查。阅读含远程图片的文章会进入该加载路径。

从当前源码直接提取未经修改的过滤函数，链接现成 `url` 依赖进行离线复现：

```text
http://127.0.0.1/a                 -> false
http://[::1]/a                     -> false
http://[::ffff:127.0.0.1]/a         -> true
http://[::ffff:192.168.1.1]/a       -> true
http://[::ffff:169.254.169.254]/a   -> true
```

已证明的是地址拒绝边界绕过；本轮没有访问内网，也没有证明任意响应外泄或 RCE。恶意文章可能让桌面向本机或内网发起 GET，响应还要经过图片解码约束。

建议：规范化 mapped IPv4；对初始请求及每次重定向解析并审核目标地址，把已审核地址绑定到实际连接，防止审核后重解析；显式处理代理路径。远程图片加载应有明确用户设置。

验收：隔离假服务覆盖 IPv4/IPv6 私有、回环、link-local、mapped、域名解析到内网、DNS 重绑定和内网重定向，确认拒绝时服务端未收到请求；正常公网图片仍可用。

### R02 依赖安全记录已超过旧报告基线

位置：[主锁文件 TLS 依赖:11433](../Cargo.lock#L11433)、[Octos 打包固定版本](../crates/hagency-app/packaging/octos.lock.json#L1)、[独立编译参数:131](../crates/hagency-app/tools/package-octos.py#L131)。

本轮 RustSec 数据库提交为 `ee1a49a2769a24a596afc1c20cb33748d2769586`，含 1,386 条公告，更新时间为 2026-10-29。结果：

| 审计对象 | 漏洞匹配记录 | 其他警告 | 解释 |
| --- | --- | --- | --- |
| 当前主 Cargo.lock | 21 条，涉及 18 个不同公告 | 45 条：unmaintained 13、unsound 28、yanked 4 | 全锁文件匹配，不是 21 个已证实可利用入口 |
| 固定 Octos revision 的独立 Cargo.lock | 47 条 | 27 条：unmaintained 4、unsound 20、yanked 3 | 独立锁文件；与主锁存在重叠，不应相加当作漏洞总数 |

主锁匹配中有 10 条对应包版本出现在本机 macOS 默认 normal 依赖图。**出现在依赖图仍不等于实际调用了受影响函数。** 对独立 Octos 的离线 feature 依赖图查询因缺少 `wreq` 索引失败；其 47 条尚不能精确映射到 `api,git,ast` 发行功能。该审计的 yanked 检查也有部分索引缺失，不能宣称完整。

优先分流如下：

| 依赖/公告 | 当前证据 | 处理建议 |
| --- | --- | --- |
| rustls-webpki 0.103.15 / 2026-0382 | 默认图含 reqwest、Matrix、updater 等路径 | 升到公告修复范围 0.103.16+，重跑 TLS/登录/更新测试 |
| webpki-roots 1.0.9 / 2026-0381 | 默认 reqwest/目录客户端路径存在 | 升到 1.0.10+；核对平台信任源差异 |
| pulldown-cmark 0.13.0 / 2026-0414 | 默认图存在；抽查 Board/article 选项未启用 WIKILINKS | 升到 0.13.5+；不能仅凭版本宣称当前 Markdown 可触发指数膨胀 |
| idna 0.3.0 / 2024-0421 | 来自 mdurl；当前 formatter 调用 percent encode/decode | 继续精确分流，不把域名鉴权影响直接套用到这里 |
| lopdf、quick-xml、zip、rsa | 不在本机默认 normal 图；部分可能经可选功能或独立 Octos 到达 | 按实际发行 features、平台及 API 调用逐项判定 |
| zstd 系列 | 部分版本在默认图；公告有 seekable、特定 streaming API、wasm32 等前提 | 分清函数/feature/平台条件，避免把 wasm32 问题称为原生桌面已可利用 |

近期 TLS 公告说明特定不再受信任的 emSign 中间证书链可能被接受，修复范围见 [RustSec 2026-0382](https://rustsec.org/advisories/RUSTSEC-2026-0382.html) 与 [2026-0381](https://rustsec.org/advisories/RUSTSEC-2026-0381.html)。PDF 深递归的具体影响见 [RustSec 2026-0187](https://rustsec.org/advisories/RUSTSEC-2026-0187.html)，它属于解析路径 DoS，不能直接扩展为整个桌面任意代码执行。

建议：主锁和独立运行时同时升级/审计；增加 PR、定时、发布前安全扫描，输出每个平台和 feature 的 SBOM/依赖清单。例外必须记录实际不可达依据、负责人和复核日期，不能统一忽略。

验收：默认 TLS 路径不再匹配上述公告；每个保留项都有适用范围；安装包所含 Octos 的 revision、锁文件、features 与审计记录一致。当前 CI 没有发现 cargo-audit/deny/vet 等持续安全门禁。

### R03 普通附件下载没有统一的内存和并发预算

位置：[sliding_sync.rs:2970](../crates/hagency-app/src/sliding_sync.rs#L2970)。

目前只对同一 MXC 去重，`get_media_content(..., true)` 在第 2985 行完整返回 `Vec<u8>`，不同 MXC 的任务表没有容量上限。保存、分享及部分导入/预览消费者走这条路径；消费者事后检查格式或体积不能约束下载阶段。相比之下，自动图片预览已有请求数、并发和 8 MiB 实收上限。

一个超大附件或连续多个下载会让内存随响应体叠加。该风险由实现确认，但本轮未做 OOM 实测。

建议：普通附件流式写入私有临时文件，增加全局并发、有限等待队列和实际接收字节预算；加密附件完整校验成功后才能向用户暴露文件。各类小型预览继续设置更小的实际接收上限。

建议验收负载：无 Content-Length、伪造小 size 的 1 GiB 响应和 20 个不同 MXC 并行请求。内存不随文件大小线性增长；取消释放连接、临时文件和额度。这里的数值是拟定验收目标，不是已测结果。

### R04 发布资格没有形成同一源码的闭环

位置：[Windows 引擎失败策略:68](../.github/workflows/rust.yml#L68)、[安装包测试:161](../.github/workflows/app-release.yml#L161)、[发布依赖:222](../.github/workflows/app-release.yml#L222)。

引擎 full matrix 的 Windows job 仍为 `continue-on-error`，注释理由是 Windows 当时不是发布目标；现在产品 README 和安装包矩阵均包含 Windows。安装包 job 测 app/contract，release 只 `needs: package`，没有绑定独立引擎矩阵的结论。因此“所有安装包成功”不等于“所有引擎验证已成功”。

[Codex 资格记录](../crates/hagency-execution/qualification/codex-sandbox.json#L1) 和 [双 Agent 资格记录](../crates/hagency/qualification/two-agent.json#L1) 仍为 `placeholder: true`；相应检查会拒绝 placeholder，CI 则按名称跳过真实资格选择器。这是明确记录的未验收状态，不应改写为通过；历史本地真实测试报告也不能自动代替这两个当前绑定记录。

建议：按支持的平台统一阻断规则；release 依赖相同源码摘要的引擎结果、安全分流和资格证明。真实模型/真实服务器资格可在受控环境独立执行，再让发布验证其版本、源码摘要、结果和有效期。

验收：故意失败的 Windows 引擎测试、过期/placeholder 资格记录、未分流的高风险依赖均不能生成可发布结论；正常 release 仍必须通过七个安装包完整性和更新签名检查。本项是流程控制缺口，不证明当前安装包已发生具体故障。

## 近期改进项

### R05 将凭证与数据库主密钥迁入系统凭证库

位置：[matrix_state.rs:32](../crates/hagency-app/src/persistence/matrix_state.rs#L32)、[会话 JSON 写入:727](../crates/hagency-app/src/persistence/matrix_state.rs#L727)、[Agent credential:574](../crates/hagency/src/console/native/agent_crypto.rs#L574)。

会话 JSON 同时保存 SDK token 和数据库 passphrase；Agent SDK 凭证也以私有文件保存。已有 Unix 0600/0700、Agent 私有存储与 Debug 脱敏，不能描述成公开凭证。问题是离线复制/备份应用目录时，密文和解锁材料可能一起取得。用户会话写入在 Windows 也未采用引擎 private 模块的显式 DACL 校验；默认 AppData ACL 是否足够、自定义宽松目录会怎样，本轮未在 Windows 实机证实。

建议统一 OS keychain/credential manager，文件只保存引用；迁移必须原子、可恢复，禁止悄悄回退明文。验收覆盖 token 刷新、多账号、凭证库锁定/不可用、中断迁移和旧明文删除，另做 Windows DACL 验收。

### R06 给分享附件的明文临时副本设置明确生命周期

位置：[attachment_download.rs:279](../crates/hagency-app/src/shared/attachment_download.rs#L279)、[temp_storage.rs:15](../crates/hagency-app/src/temp_storage.rs#L15)。

分享会把 SDK 返回的字节（包括已解密附件）写入账号 hash 下的缓存 temp。普通 `create_dir_all/fs::write` 未主动强制私有权限；失败、取消或登出没有该副本的及时删除路径。清理仅在下次启动 60 秒后删除启动前的旧文件。hash 命名提供隔离，不提供加密。

建议使用私有临时对象，按平台分享完成生命周期清理，辅以 TTL、登出/退出和崩溃重启兜底，并拒绝链接替换。验收覆盖成功、取消、失败、长期开启和登出；跨用户可读性必须依据实际权限测试，不凭 umask 推断泄漏。

### R07 上传改为实际字节受限的流式读取

位置：[sliding_sync.rs:2574](../crates/hagency-app/src/sliding_sync.rs#L2574)、[全量读取:2589](../crates/hagency-app/src/sliding_sync.rs#L2589)、[服务端限制查询:5645](../crates/hagency-app/src/sliding_sync.rs#L5645)。

上传先检查此前保存的文件 size，再 `tokio::fs::read` 完整读取；文件可在期间增长。服务端限制查询失败会继续上传，没有独立的应用内存准入。建议在实际文件句柄上复核，读取过程限制实际字节，优先 SDK 文件源/受控流，并限制并发。验收在选择后扩大文件，以及大文件上传、取消和跨房间并发，不让拒绝发生在完整缓冲之后。

### R08 下载回调应先释放全局锁，再执行同步操作

位置：[sliding_sync.rs:2995](../crates/hagency-app/src/sliding_sync.rs#L2995)、[分享回调:252](../crates/hagency-app/src/shared/attachment_download.rs#L252)。

`if let download_tasks.lock().unwrap().remove(...)` 的成功分支执行回调时仍持有 Mutex guard。分享回调同步写完整文件并调分享接口；其他下载注册、检查和取消也争用此锁。Tokio worker 最多 4 个，慢磁盘或分享接口延迟可能阻塞无关请求；本轮没有证明必然死锁。

建议用独立作用域取出任务后释放锁，文件写入/分享准备交给有限 blocking 池；取消路径一并处理。验收可注入 5 秒慢写，证明其他下载取消和派发仍及时响应，且回调期间 `try_lock` 可成功。

### R09 资料缓存和完成更新要纳入统一资源预算

位置：[user_profile_cache.rs:18](../crates/hagency-app/src/profile/user_profile_cache.rs#L18)、[更新队列:70](../crates/hagency-app/src/profile/user_profile_cache.rs#L70)。

用户 BTreeMap 和每用户 room-members map 没有容量/TTL/逐出；完成队列是无界 SegQueue，UI 一次排空。遍历大群和长历史时，资料关系随遇到的用户持续增长，集中返回可能造成单次 UI 处理尖峰。

建议用可观测的容量/LRU，保护当前可见集合，按用户/房间合并更新；沿用时间线已有的有限条数/约 4 ms UI 批次思路。验收以 10 万用户/关系的合成负载检查容量封顶、更新完整性及输入 p99；容量数字须由产品负载设定，不能直接照搬为已测保证。

### R10 让历史索引追赶在房间之间公平调度

位置：[history_store.rs:154](../crates/hagency-app/src/history_store.rs#L154)、[room_history.rs:790](../crates/hagency-app/src/home/room_history.rs#L790)。

后台按房间串行 await；每个房间每页 100 条，循环追到末尾才让出，并全程持有哈希分片锁。大量离线积压的房间会拖延其他房间的搜索索引，分片碰撞还可能影响前台追赶。

建议每轮限制页数或时间，保留持久 cursor 与撤回顺序，同房间串行、跨房间有限并发并显示索引滞后。验收：A 有 10 万条积压，B 仅一条编辑/撤回，B 应在拟定的约 2 秒目标内更新；A 最终完整追平。不能通过跳 cursor 或漏撤回换取速度。

### R11 保护快速新建卡片草稿

位置：[quick_create.rs:34](../crates/hagency-app/src/hagency/board/quick_create.rs#L34)、[can_switch:509](../crates/hagency-app/src/hagency/board/mod.rs#L509)、[下拉切换:180](../crates/hagency-app/src/hagency/ui/project_navigation.rs#L180)、[绑定清理:97](../crates/hagency-app/src/hagency/board/controller.rs#L97)。

列内 Add card 后输入标题但尚未保存，quick-create 只设置独立状态，没有设置 dirty；导航 can_switch 只检查 dirty/pending/modal，因此允许切 Board，之后清理函数清空标题。任务详情 modal 本身有背景输入保护，不属于这里的正常触发场景。

建议统一离开保护或按账号/Board/列保留草稿。验收中文输入后经下拉、侧栏、Project 和隐藏返回都保留或明确确认放弃；换账号不得串草稿。

### R12 审批应具备跨页面的发现入口

位置：[ui.rs:1475](../crates/hagency-app/src/hagency/ui.rs#L1475)、[轮询前提:1528](../crates/hagency-app/src/hagency/ui.rs#L1528)。

当前只有打开本机 Agent 的运行详情页才轮询审批，新增提示依赖这次读取，且仅为 8 秒 popup。用户在 Board、其他 Room 或后台时可能不知道任务在等待。此为产品闭环缺口，不能以自动批准解决。

建议账号级有界订阅/轮询，提供全局 badge、待办和可选系统通知；决定时继续重验原请求。验收覆盖聊天、看板、最小化及换账号，通知只打开精确请求、不授予权限。

### R13 接通已有历史能力与 Matrix 永久链接

位置：[room_screen.rs:3082](../crates/hagency-app/src/home/room_screen.rs#L3082)。

RoomAlias 和 Event 链接分支只记录 TODO 并返回 false，随后落到浏览器/系统 URI。已有房间和历史 event-context 能力没有被链接入口复用。建议支持 `matrix.to` 与 `matrix:` 的 alias 解析、via、原生房间预览和事件定位；已加入旧消息应定位高亮，无权限/删除/不存在时有明确反馈。

### R14 明确音视频能力并补齐可用操作

位置：[音频:6895](../crates/hagency-app/src/home/room_screen.rs#L6895)、[视频:6939](../crates/hagency-app/src/home/room_screen.rs#L6939)。

时间线目前展示元数据占位而非播放器；音频还显示“Video playback”未支持的提示。附件浏览器已有下载/分享，因此不能说媒体完全不可访问。建议先提供清楚一致的下载并打开操作和准确文案，再实现音频播放/暂停/进度及视频。验收覆盖普通/加密媒体、取消下载、切账号停止和外部打开失败。

### R15 同步加密能力、快速开始与隐私声明

位置：[README.zh-CN.md:62](../crates/hagency-app/README.zh-CN.md#L62)、[privacy.zh-CN.md:19](../crates/hagency-app/docs/privacy.zh-CN.md#L19)、[当前 encrypted 执行入口:1246](../crates/hagency/src/console/owned_runtime.rs#L1246)。

中英文 README、quickstart、privacy 仍整体描述 Agent 加密执行未开放；当前 production 已接入 encrypted prepare/start、reply 和 files，协调记录也有 owner 链的历史实际证明。应分别列主人私聊、共享 Room 请求者、跨主机恢复、平台与生产登录资格，不能继续全部写延期，也不能反过来称所有加密场景均支持。

建议维护一份可追踪能力清单，让文档共用或校验它。每个声明绑定代码与资格证据；用户应能知道哪些数据会发给模型、哪些链路加密、何时需要恢复。

### R16 修正低风险契约错误并逐步缩小热点模块

[server_admission.rs:94](../crates/hagency/src/server_admission.rs#L94) 校验 protocolVersion 必须为 3，但第 152 行返回 `protocol_version: 2`，第 293 行测试也固化这个错值。当前抽查消费者没有使用该字段，因此不认定为登录阻断；应使用同一个协议常量，测试对照输入真实版本而非重复错误实现。

模块边界是项目优点，但 `room_screen.rs`、`sliding_sync.rs`、管理 `ui.rs` 等仍各有数千行，包含输入、渲染、网络调度和状态转换。文件长度本身不是 bug，建议随 R03/R08/R11 的实质修复提取有明确所有权的传输器、草稿/导航状态和审批订阅，避免一次性大重构。迁移验收应以现有行为和故障语义等价为准。

## 功能完成度矩阵

| 能力 | 当前判断 | 下一步验证或补齐 |
| --- | --- | --- |
| Project、Space、Rooms、成员、Access | 实现较完整 | 管理员/普通成员/失权/部分读取失败组合 |
| 多 Board、任务、拖放、评论、清单、附件 | 功能面丰富，版本冲突/幂等/未知写设计较好 | 快速创建草稿；离线原请求恢复；多用户冲突 |
| 聊天、线程、历史搜索、附件浏览 | 有实质实现 | 消息链接、音视频、超大附件与取消 |
| Agent 身份、设备绑定、模型/MCP、三层额度 | 实现深度较高 | 多 Room 并发、失权、账号切换、资源恢复组合 |
| 本地/远程审批与未知执行 | 精确作用域及新鲜度设计较好 | 全局发现；真实用户恢复演练；后台体验 |
| 主人加密执行与回复文件 | 当前生产路径已接入，有历史实际证明 | 当前版本完整桌面验收；精确区分共享请求者/恢复限制 |
| 跨平台更新 | macOS Sparkle、Windows 更新状态机、Linux 手动路径 | 最终安装包上实机升级/失败/取消/回滚拒绝 |
| 无障碍 | 已有部分焦点范围、Tab/Escape 实现 | VoiceOver/NVDA、200% 缩放、仅键盘关键流程；本轮不作合规结论 |

不建议用“完成 90%”这类单值。应对每项分别记录：实现存在、自动测试通过、原生操作验收、真实服务验收、平台验收、已知限制。不同证据不能相互替代。

## 建议实施顺序与验收标准

| 批次 | 工作 | 完成条件 |
| --- | --- | --- |
| A 发布前安全与资源阻断 | R01、R02、R03、R04 | 图片请求边界负例通过；TLS 更新且双锁分流；大附件有界；发布绑定同源码资格 |
| B 数据保护与响应性 | R05–R10 | 凭证迁移/临时文件生命周期明确；慢写不阻塞；缓存封顶；索引公平 |
| C 日常协作闭环 | R11–R15 | 草稿不丢；审批可发现；permalink 可用；媒体操作明确；能力文档一致 |
| D 持续质量 | R16、性能回归、无障碍 | 契约常量一致；热点职责逐步收敛；指标和平台验收可重复 |

可并行分为安全存储/网络、附件传输、Board/审批交互、发布测试四条工作流。附件安全与性能共用传输层，应由同一负责人维护预算、加密校验、取消、临时文件所有权，避免各消费者重复实现。

建议在当前已有长测工具上扩展：真实多房间成员、普通附件大流量、慢磁盘、离线积压及账号切换；记录内存/RSS、UI 输入 p99、活动请求和实际字节、索引滞后、审批等待时间。历史 2026-10-21 的长测是工程基础，不是当前 HEAD 全客户端的内存保证。

## 本轮验证与证据范围

原始日志、锁文件摘要和审计 JSON 保存于 [.run/quality-review-20261029](../.run/quality-review-20261029)。该目录被 Git 忽略；需要随报告外发时应另行打包证据。测试只在本地隔离 fixture 范围运行。

- `cargo test --locked -p hagency-app --lib`：706 通过、0 失败、2 原有忽略，执行 71.14 秒；不等于全 workspace 或所有 feature 通过。
- `cargo test --locked -p hagency-store -p hagency-history -p hagency-updater --lib`：history 8 通过/2 忽略，store 105 通过，updater 4 通过，均 0 失败。忽略项要求真实百万/千万数据库压力环境，本轮未运行；三库共 117 通过，不代表完整引擎集成测试。
- 主锁新鲜 `cargo audit --json`：21 条漏洞匹配；独立 Octos 锁 `--no-fetch` 审计：47 条，索引/yanked 完整性限制见 R02。
- 当前远程图片过滤函数离线探针：mapped IPv4 拒绝失败，结果保存在 `remote-image-filter-probe.log`。
- `node --test` 两组 production-callers/spec-bindings 自测试：46 通过、0 失败。这是门禁实现的自测试，不是全部 spec 绑定库存或真实业务资格验证。
- 工作区边界：30 包通过；样式：306 文件、0 违规；主题字面量检查通过；i18n：2,715 词条、1,981 个翻译调用点、0 缺失。
- 包装脚本测试：package-app 3 项、package-octos 6 项、release-assets 2 项通过；不是实际安装包的安装/启动验证。

本机默认 Python 缺少 `tomllib`，导致首次工作区检查和 package-app 测试失败；使用现有 Python 3.12 重跑后通过。过滤探针首次链接的依赖由 Rust 1.99 编译，Rust 1.98.1 无法链接；随后用现有 1.99 工具链完成隔离探针。桌面库测试本身使用 Rust 1.98.1；这些环境差异不被改写为产品失败或 CI 等价结果。

本轮没有重新做当前版本 GUI 全流程、真实 OAuth 登录、动态渗透、真实模型调用、Windows/Linux 原生长测或读屏验收，也没有完整运行所有 workspace/feature 测试。根模板要求的 `task-writer` 在这个源码 checkout 未 provision；没有创建替代任务真相。`specs/project.spec.md` 不存在，评审未修改实现，也未将 skip/uncertain 算成通过。

项目应继续保留的设计包括：账号任务 epoch 与 abort/drain、私有文件存储的 owner/权限/链接检查、严格审批原请求校验、未知结果与账本保留、更新 manifest/安装包验签、时间线窗口与图像/GPU预算、加密历史索引和原始失败证据。后续修复应扩展这些现有机制。
