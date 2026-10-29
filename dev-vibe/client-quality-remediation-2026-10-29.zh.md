# Hagency Client 质量改进执行记录

本记录对应[完整评审](client-quality-review-2026-10-29.zh.md)。用户已授权分批修复、提交 PR，并在代码复核和当前版本 CI 均通过后合并。整体目标仍有效；本页是协调与证据记录，不替代运行时任务状态。

第一批从 `origin/main` 的 `932dcdea` 开始，位于独立工作树；未混入已有的 tutorial PR 或原工作区未提交材料。Task Contract 为 [task-client-quality-hardening.spec.md](../crates/hagency-app/specs/task-client-quality-hardening.spec.md)。

## 当前交付状态（2026-10-30）

主整改及原生 Matrix 链接已通过 [PR21](https://github.com/hagency-org/hagency-client/pull/21)
统一合入 main：实际验收 head 为 `48b33772c1a2bbe143b8fc5a869dd676c19c327e`，
合并提交为 `cf7fbb63b45c751d7f7efc0369963de3839435b5`，时间
2026-10-29 23:03:32 UTC。父复核逐项确认当前 head 的 15 项实际检查均成功，
包含五个桌面构建、两架构 macOS App、三平台原生引擎、fast Rust、portable
article、拼写、scope 及汇总。四项仅手工触发的移动构建和夜间 issue 维护按条件
跳过，不计通过或移动资格。PR18–20 的提交已包含在该 head 中，随祖先关系自动
标记合并；原 head 的失败、取消或未完成记录均保留，验收依据是上述实际完整提交。

父评审独立下载该 head 的 Linux workflow `40958705871`、job `123745783208`
原始附件测量：八种实际 plain/encrypted、32/256 MiB、1/4 并发组合完整，
12 个源码哈希与提交逐个相等；逐个重算 raw heap、检查进程采样并重跑生产验证器，
均无错误。最大 Rust 堆 3,506,028 B，采样进程 RSS 33,034,240 B；范围仍仅为
SDK 初始化之后的隔离 remote transport，不是整个 App、TLS 或 SQLite 的资格。
合并源码的父独立全 App 为 824 通过、0 失败、2 个原有忽略，精确 CI 特性 own
严格 Clippy 和 Matrix 链接完整 lifecycle 8/8 通过。

SDK PR2 已合并，当前主整改仍固定其已审祖先 a70f。后续凭据所需 SDK PR3 的
弱实例比较已在 13 项 current-head CI 及六份新鲜 benchmark 的 50 项 profile
复核后合并；只读身份 API 的 SDK PR4 也已在父独立读库 8/8、完整加密模块 15/15、
三平台在内的 13 项当前检查成功及六组新鲜 benchmark 的 50 项命名测量复核后合并
为 `d325f930989d65044c52fcabcb2b7e66d533440a`；客户端可固定已审祖先 `95de9a4e`。
SDK 原样 lifecycle 的 reader 因工具不能选择 features 而 Skip，未计作通过。
R05 桌面迁移及 R07 SDK 上传资源控制尚未合并；
移动凭据、真流式上传、剩余依赖及内置播放器等开放范围见清单。下文各轮日志是
保留的历史记录，不将其当时的“待验证”或失败改写成已通过。

## 完整改进清单

| 编号 | 目前状态 | 交付与剩余工作 |
| --- | --- | --- |
| R01 图片请求内网边界 | 已合并 PR21 | 每跳审核全部 DNS 地址并绑定连接，关闭代理和自动跳转；保留时间、字节和解码预算 |
| R02 双锁依赖与审计 | 部分修复，继续开放 | 主锁匹配 21→12→11，audit 仍失败；独立 Octos 的 47 条为旧记录，本批未重审计，不能当新鲜数量；剩余源码迁移及持续安全门禁开放 |
| R03 普通附件下载预算 | 已合并 PR21，八组实测通过 | 私有文件流式传输、4 操作/16 排队、字节及磁盘额度、取消、TTL、完整性和原 IO 归属；不把测量窗当整个 App 内存资格 |
| R04 发布资格闭环 | 门禁已合并 PR21 | 三平台引擎成功及同源码真实资格均为公开发布前置；当前占位资格仍正确阻止公开发布 |
| R05 系统凭证库 | 桌面实现与复核中 | 小型原密钥与包装密钥进系统库，大会话/撤销 JSON 认证加密；SDK 只读身份前置已合并，owned handoff 与恢复准入待最终整合；真实 OS 资格未取得，移动 v1 尚未修复 |
| R06 分享明文生命周期 | 已合并 PR21 | 私有句柄、30 分钟 TTL、预算、退出及崩溃回收；原生分享无完成回调，已交付外部副本无法撤销 |
| R07 流式上传 | App 准备边界已合并；SDK 有界缓冲验证中 | 原文件身份、64 MiB 单源/128 MiB 准备预算已交付；SDK 共享 worker/队列、真实资源测量及未知结果确认重试待完成；真正流式架构仍开放 |
| R08 下载完成锁与阻塞 | 已合并 PR21 | 回调前释放锁，有限后台准备/写盘/门户观察，原 IO 保活及账号消费围栏 |
| R09 资料缓存 | 已合并 PR21 | 用户、成员和完成队列有界，合并与有限 UI 批次，容量拒绝延期重试 |
| R10 历史索引公平性 | 已合并 PR21 | 两个有限 page worker、FIFO 轮转、精确房间锁和原 cursor 事务；已进入 SQLite 的原 IO 不被伪装取消 |
| R11 快速建卡草稿 | 已合并 PR21 | 按 Board 保留原文与未知写幂等键，32 个草稿显式容量，保留取消/确认保存/换账号语义 |
| R12 全局审批发现 | 已合并 PR21 | 常驻入口、账号级有界分页及 badge，只观察现有 owner；路由后复核请求；系统通知未实现 |
| R13 原生永久链接 | 已合并 PR21 | matrix.to/matrix:、alias/via、事件/内层 thread 上下文、确认加入及实际 UI scope 检查；系统 URL 启动未资格验证 |
| R14 音视频操作 | 原操作、指引与实际点击回归已合并 | 原时间线及历史已有下载/分享/另存为；内置播放器仍未实现 |
| R15 加密能力声明 | 已合并 PR21；凭据文档后续更新中 | 中英 README/quickstart/privacy/release notes 区分 owner、shared、恢复及真实资格边界 |
| R16 契约与模块热点 | 协议修复已合并，结构持续改进 | discovery 校验及返回均为 3；大型模块的职责收敛仍持续，不声称已整体重构 |
| R17 worktree 重复编译 | 已合并 PR21 | 只监视真实 Git HEAD/ref/packed 及 linked 指针；重复选择器已复用产物，源码 archive 不虚构路径 |
| R18 SDK 元数据响应字节上界 | 新发现，开放 | SDK 通用响应仍整份 bytes 缓冲；上传 POST 的增量限长拟入 R07，versions/media config/encryption state 等前置元数据响应仍需独立处理；受控测量不能证明敌对响应硬上界 |
| R19 自动数据库清理跨进程边界 | R05 实施安全停止，完整空间回收开放 | 启动盘点与另一进程创建未统一串行，旧快照会把新 DB 当孤儿；生产自动删除暂停，保留数据可能继续占盘。未来统一跨进程协议及资格完成后才可恢复自动删除 |

## 第一批实现与复核

图片请求不再仅审核 URL 字面地址：解析每一跳的 DNS，拒绝任何不安全候选，使用固定地址连接并保留原 Host/TLS 主机名。所有重定向由同一校验循环处理；整体20秒deadline包含DNS与响应读取。七项测试包含生产逻辑的隔离输入以及真实本地HTTP传输对地址绑定、Host和不自动重定向的验证。

草稿保留由Board绑定生命周期统一处理，因此下拉、侧栏与Project导航共享行为。未知保存保留原幂等键；迟到旧页面回执不能确认新页面状态。最多32个并存草稿；达到容量时解释如何返回已有Board处理，不逐出已有输入，也不禁止导航。测试覆盖未取得快照前快速离开、隐藏返回、列删除、取消、账号变化及原请求重试。

资料缓存设置2048用户、8192总成员关系、每用户64房间，完成队列512个合并槽，UI每次最多32项/4毫秒。独立复核发现并要求修正“延期Signal但空队列不重绘”的边界；已补充生产计时器到实际重绘的回归，最终全App验证通过。

主锁更新 ctutils、pulldown-cmark、rustls-webpki、webpki根证书和 zstd 兼容版本。审计剩余12条包含可选/独立运行时及需要函数适用性判断的包；没有新增统一豁免，也没有把47条独立锁匹配算作已修复。

安全实现由另一位agent独立复核，草稿实现自行复核，主评审检查调用链和状态所有权。最终合并仍要求当前head全部平台job实际成功；现有continue-on-error不能替代该检查。

## 验证记录

- 新鲜图片相关选择器14通过，其中新增网络边界7项。
- admission选择器4通过、1项要求真实可信HTTPS服务器的原有忽略。
- 第一轮全App719通过、1失败、2原有忽略。失败在新增LRU测试：所有插入使用同一时间，断言固定ID最旧而未考虑有序映射的同时间排序；要求改为真实递增时间以检验LRU，不更改生产逐出规则迎合断言。原始失败日志保留。
- 首轮App/engine own all-targets严格Clippy通过；补充修复后的最终检查单独记录。
- 翻译2718词条、0缺失；主题、306文件样式、30包工作区边界及46项门禁自测试通过。
- agent-spec1.4.0已安装到本次忽略的工具缓存，Task Contract parse通过，lint得分0.963。lint提示人工CI复核约束没有机器scenario；不将该提示当作实际CI验收。完整lifecycle及最终结果随后补充。

原始输出位于本工作树的 `.run/quality-hardening/`。缺少 provisioned `task-writer`，未创建替代任务真相。没有更改真实账号、部署或启动付费模型。

## 第一批最终本地复核

- 最终冻结源码全App库：721通过、0失败、2原有忽略，执行81.83秒。
- 最终App与engine own all-targets严格Clippy通过；admission4通过/1原有真实HTTPS资格忽略。
- 独立安全复核确认profile计时器due、账号epoch与取消、空queue重绘、overflow恢复和容量检查无阻断问题；图片网络实现由另一位agent独立复核，无阻断发现。
- 这不替代后续当前PR的各平台CI、实际安装包或整体16项改进完成证明。

- 首次完整lifecycle为4测试scenario通过、1边界失败：agent-spec对根目录无斜杠的`Cargo.lock`边界未匹配；将同一精确路径规范为`./Cargo.lock`后重新核验，未扩大允许文件范围。原始非通过JSON保留。

- 规范路径后的完整agent-spec lifecycle最终5通过、0失败、0跳过、0不确定，包含精确边界和全部4个绑定测试scenario。
- 另发现构建脚本在Git worktree中监视不存在的`.git/HEAD`，会反复触发重编译；已核实真实HEAD/ref路径由`git rev-parse --git-path`给出，列入下一批构建效率修复，不混入当前已冻结实现。


## 第二批实现与验证中（2026-10-30）

Task Contract 为 [task-client-quality-followups.spec.md](../crates/hagency-app/specs/task-client-quality-followups.spec.md)。此批在独立后续分支实现；第一批 [PR18](https://github.com/hagency-org/hagency-client/pull/18) 保持冻结，尚未合并。

- 发布流程复用三平台引擎检查，移除 Windows continue-on-error；公开发行要求实际资格记录及其源码、版本和 Rust 判据同时通过。直接编入程序的 Agent Markdown 模板、安装资源、配置和工具链均属于资格输入。四项 Python 历史仓库自测试通过；真实记录仍为 placeholder，预检查按设计拒绝，不伪造通过。六份 workflow 的 actionlint 通过。
- 分享明文使用原有私有存储权限和 nofollow 文件身份检查、独立进程租约、30分钟TTL、32文件/256MiB暂存预算、8个排队工作。原线程保留清理和外部观察归属；删除失败仍占预算，账号退休和退出阻止旧呈现。平台API没有成功分享完成回调，TTL及外部副本限制保留。Linux portal认证、准备和响应观察在后台，UI只执行单次非阻塞发送；可能已发送的失败不自动再次启动。Windows延迟数据请求也重新检查原账号围栏。vendored robius-share保留原MIT声明及原ShareSheet语义，说明见HAGENCY-PATCH.md。
- 下载注册表在调用完成/取消回调前移除原回调并释放mutex，回归覆盖回调重新进入注册表。此项不关闭R03普通下载仍先完整缓冲的问题。
- 历史索引新增8项真实SDK/本地HTTP/加密索引回归通过，包括旧hash碰撞房间的独立推进、1200行重放与重开、取消后的原IO持锁和账号退休。首次7通过/1失败保留：测试错误地认为健康索引reset_stale会清数据，修正为先标记stale，再验证原generation/coverage行为，没有改变生产语义。
- 引擎待审批两项生产路径测试通过：80条跨两个Agent的分页、原NativeOwner请求与账本/HTTP计数、游标退休拒绝；没有启动provider或新lease。App首次全库749通过/3失败/2原有忽略。三项失败揭示虚拟列表未绘制、500px侧栏footer溢出，以及SDK restore_session后台初始化混入观察测试。实际绘制与预算修复正在重新验证，原失败保留；不把尚未通过的UI声明为完成。
- R17构建效率：通过git rev-parse识别普通/linked worktree、packed refs及detached HEAD，监视实际存在的原Git路径与linked指针文件；源码archive不虚构路径。三项实际Git仓库测试通过，待全App一起最终核验。
- 本机macOS vendor库8项测试通过；Windows及Linux vendor库（后者含测试目标）交叉类型检查通过。这些不是实际Windows分享UI或Linux桌面portal的资格证明。Android/iOS guarded share及Linux混合save bundle明确不支持。
- 当前静态检查：310份源文件样式0违规、2729词条/0缺失、主题及31包工作区检查通过；Task Contract parse通过、lint98%。完整lifecycle、最终全库/Clippy及第二批PR仍待完成。

本批原始输出保留在 `.run/quality-followups/`。第一批托管桌面构建均成功，完整引擎Windows还在运行；macOS引擎任务因托管运行器容量不足未取得runner而取消，不能算作通过，将在该workflow结束后选择性重跑。


### 第二批冻结源码结果

完整App库最终752通过、0失败、2原有忽略，88.26秒；包含原500px侧栏、6项审批widget、私有暂存及8项历史公平性回归。App/engine/vendor own all-targets严格Clippy通过。额外Linux vendor含测试目标的严格Clippy最初报告继承代码可用`?`简化分支，保持相同None语义改写后通过；没有加lint豁免。此交叉检查仍不能替代实际Linux门户测试，实际Linux/Windows库测试已加入对应托管构建lane。

独立依赖梳理确认主锁剩余风险不能全部通过兼容cargo update消除：idna被mdurl旧约束锁定；quick-xml、zip/zstd和lopdf需要Octos源级迁移；crypto-bigint涉及上游加密API栈；rsa公告仍没有修复稳定版。独立Octos锁有一批同semver更新候选，但其减少数量只是公告集合计算，未求解/编译/重审计，不计作修复。后续必须更新真实源码及打包pin，不能修改工具缓存冒充提交。

- 第二批完整agent-spec lifecycle：7通过、0失败、0跳过、0不确定，包含边界及六组原始绑定测试；lint0.981。Python及actionlint另行通过，不将Cargo-only lifecycle夸大为这些非Cargo检查。

- R17在不改源码的原Cargo选择器重复执行时直接复用产物，2.36秒完成构建阶段，三项Git测试0.09秒；未再出现原worktree不存在路径导致的每次完整重编译。


### 第二批平台CI发现与修复（2026-10-30）

原current-head托管Windows App全库739通过/7失败/2原忽略，Linux751通过/1失败/2原忽略，原失败保留。Windows六项分享失败定位到真实目录句柄：cap-std有意不允许directory的FILE_SHARE_DELETE；clone.remove_open_dir关闭的是clone，原目录仍被StagedFile持有。现仅在确认原文件unlink后关闭真实child句柄，通过保留的原parent删除空目录，失败保留parent重试；不会递归遍历replacement。新增真实文件/目录删除故障及replacement保留回归，本机temp_storage13项通过，实际Windows仍待新head CI。

Windows另有Git测试把canonicalize产生的verbatim盘符路径传Git导致拒绝，测试fixture现保留OsString转换回普通盘符/UNC，且Git失败完整记录status/stdout/stderr；保留真实linked worktree。Linux锁失败与并行fork继承open file description相符：本机Unix受控fork证明，即使FD有CLOEXEC，父drop后子exec/close前锁仍在，子关闭后立即可取；未捕获原Linux具体继承者，不夸大为已在Linux复现。两个真实store lease测试使用同exe单test子进程隔离无关fork，所有held拒绝、原drop、scope与symlink断言原封不动，没有重试、sleep或修改生产锁。新Linux CI仍是最终平台证据。

- 平台小补冻结源码本地完整App753通过/0失败/2原忽略，69.70秒；own App all-targets严格Clippy及agent-spec lifecycle7通过/0失败/0跳过/0不确定，样式310/0通过。Windows专属Git测试已通过Windows target rustc metadata类型检查，真实运行仍待托管Windows。第一批macOS第二次选择性重跑仍未取得runner并取消；不算通过。PR19已包含第一批全部提交，现作为统一整改候选，要求其最终head所有平台job真实成功后再合并，保留中间版本原始失败/取消记录。


- 后续Linux两架构暴露审批observer测试setup只收到1条而非2条GET。原SDK初始化屏障仅等待主setup；Recovery还有读取初值Unknown的长期观察任务，第二次初始化读取可稍后才发生，并不是UI启动owner。测试现以真实server完整回复两条白名单GET的Notify条件作初始化屏障，再保留观察阶段零新增HTTP、startup0、CURRENT为空和无owner目录的严格断言；不以sleep静止或allow任意请求迎合结果。本次6项审批测试和own App全targets严格Clippy通过，lint/boundary两层lifecycle通过；此前完整753/0/2与7项完整lifecycle仍保留，未把本次选择性复核改写为新的完整运行。当前head各平台CI仍需通过。


## 第三批实现与验证中（2026-10-30）

Task Contract 为 [task-client-quality-attachments.spec.md](../crates/hagency-app/specs/task-client-quality-attachments.spec.md)。普通下载采用真实认证HTTP与私有文件、固定4worker/16元数据排队、64KiB自有IO缓冲、1GiB单文件及每原生进程4GiB实际临时文件额度。密文先完整hash验证再解密发布；Matrix AES-CTR不能单独检测“格式合法但错误的密钥”，依赖Matrix事件元数据完整性，不伪称有此能力。Theme/Markdown分别在256KiB/512KiB以内才能读入内存。Save As保留显式原生选择，目标旁scratch通过原父/文件身份和恢复日志清理；已发布用户文件不删。

取消、账号退休、原始阻塞IO、SQLite缓存读取和原生选择回调各自保留真实任务归属及预算，不能仅因观察future被丢弃就释放。空闲下载记录只保留SDK Weak，避免旧SDK/数据库被持有。会话失效及主题/Markdown导入在真实UI消费处重新检查原epoch与client；SESSION_IO等待不阻塞Tokio线程。状态重试最多3次，保留429/可恢复5xx和Retry-After，M_UNRECOGNIZED不重试；不新增发送/读取网络错误重试掩盖fixture失败。

此前SDK b218源码编译通过，36项附件/上传/原生选择/主题/会话/真实媒体点击定向测试全部通过。完整App首次787通过、1失败、2原有忽略，143.48秒；失败在历史大房间12页全部完成5秒聚合deadline，小房间抢先和实际索引断言已过，独立同selector4.18秒通过。正在核查负载与真实提交进展的等待方式；没有将选择性通过改写为完整通过。独立复核另发现create_new后权限设置失败可能遗留空文件/空恢复日志，正在补失败清理与故障注入。

内存测量使用真实生产transport及32/256MiB明文/密文×1/4并发八个独立进程，预先固定Rust分配峰值32MiB、输入放大峰值增量8MiB。release程序还在构建，尚无通过数据；Linux CI已绑定同一真实测量并保留JSON/heap/进程/文件证据。SDK本地缓存的native SQLite分配由依赖侧单独核验，远程传输测量不能替代该路径。翻译2729词条零缺失、主题/样式检查已通过，最终lifecycle、严格own-Clippy、完整App和当前head各平台仍待完成。


第三批后续复核补充：依赖SDK a70f68e的同事务预检先检查SQLite真实BLOB类型与字节长度，再更新last_access。实际bundled SQLite验证128MiB超限BLOB/含NUL TEXT拒绝时新增原生分配为0；接受64MiB项的额外native highwater观测为268,599,296字节，不能将Rust129MiB scratch声明为SQLite/RSS上限。新SDK三平台均通过完整SQLite32及加密存储12项，全部当前head CI仍待结束。

图片预览原先同样先读整个缓存再限长，现与普通本地下载共用1个原始SDK cache worker、16元数据排队、64MiB读上限；普通本地下载另保留原Rust scratch admission直到返回Vec实际写盘完成。预览缓存写入先预留32MiB排队/活跃clone总额再复制；原SDK future直到实际完成才释放，取消不伪装成原SQLite interact被取消。SQLite native、SDK编码和调用者结果另计，不将这些限定夸大为全进程129MiB。原来以MediaStoreLock充当同进程mutex的3项测试夹具失败已保留；SDK此锁实为可共享的跨进程租约。新夹具在实际MediaStore get/add future内受控等待、委托MemoryMediaStore，真实SQLite格式/超限测试另行保留，不冒称夹具证明了原生SQLite阻塞。

当前新源码定向复核：附件26、cache reader5、preview4、scoped popup3、历史公平性8项均通过。创建/权限设置失败的原句柄见证与删除未知计数保留到真实完成；并发retry不会使诊断归零，也不在UI诊断锁中执行文件IO。附件popup原epoch在实际出队及已显示widget处复核，换账号会移除旧文件名诊断。历史测试保留小房间抢先2秒与真实12页1202行/原游标/重开检查，将总5秒聚合等待改为逐真实提交的5秒无进展watchdog并硬限13轮，未减少业务工作或修改生产调度。首次完整lifecycle4/0/0/0仍单独保留；新增加的cache/popup场景及最终全库/Clippy/八组实测尚待最终运行。


### 第三批冻结源码本地结果

最终完整App802通过、0失败、2原有忽略，77.15秒；own App all-targets严格Clippy（含示例）通过；完整agent-spec lifecycle7通过、0失败、0跳过、0不确定。320份源文件样式零违规，2729词条零缺失，主题、31包边界和修改workflow的actionlint通过。补齐Theme/Markdown动作字段时保留同步示例的原epoch；只公开只读epoch accessor，不公开切换或复位接口。

| 传输 | 并发 | 32MiB项Rust堆峰值 | 256MiB项Rust堆峰值 |
| --- | --- | --- | --- |
| plain | 1 | 1,138,965 B | 876,888 B |
| plain | 4 | 3,479,212 B | 3,475,096 B |
| encrypted | 1 | 877,713 B | 869,524 B |
| encrypted | 4 | 3,482,524 B | 3,478,408 B |

八个独立进程全部输出校验、真实并发、私有文件清理及quota归零通过；固定32MiB绝对峰值/8MiB输入放大增量阈值未调整。最大Rust堆峰值3482524字节；采样全进程RSS最大31522816字节。仅证明该隔离HTTP fixture的生产remote transport测量窗口（SDK初始化在Rust计量窗外），不代表完整App基线、TLS、系统文件缓存或SQLite本地读的全进程内存资格。manifest保留binary/source/lock/driver SHA与基线Git HEAD，冻结工作树的未提交源码通过逐文件hash标识；提交后仍需当前head CI。最终原始数据见`.run/quality-attachments/remote-memory-a70f-final/`，原始失败及先前诊断/迭代未覆盖。

一次未加--no-deps的全依赖严格Clippy还发现原有article-makepad27项lint；原始日志保留，不能据本次own App检查宣称全依赖严格Clippy已通过。独立安全与性能复核确认本批核心原IO、scope、TTL、创建失败及真实测量路径无已知阻断；SDK PR2、第三批客户端PR和当前head各平台CI尚需最终通过后才可合并。R02剩余迁移、R05凭据库、R07 SDK总队列、R13及R14内置播放器仍开放。


### 合并候选复核更新（2026-10-30）

SDK PR2 的当前 a70f 提交13项检查全部成功，已独立复核并合并为709ad8ec；
Client继续固定已审查的a70f祖先。客户端统一PR20的361提交全部五个桌面构建、
三平台engine、ARM App及拼写已通过；Intel App的808通过/1失败/2忽略保留为失败。
失败发生在媒体HTTP测试服务器404，原日志没有记录完整请求路径，不能归因到
已捕获的具体包阶段。修复测试服务器的原假设：accepted socket显式blocking，
完整且受限的header才路由，Interrupted继续、不完整及读取失败直接拒绝；
真实sync路径支持有无query，记录最近32个完整method/path供诊断。原实际控件
点击、明文/加密、文本复用断言均保留，新分片/中断/不完整读取回归通过。

新组合包含已审查的原始receive receipt夹具和R02 IDNA迁移。精确CI特性完整App
810通过、0失败、2原有忽略；同特性own all-target严格Clippy通过。真实SQLite
receipt2项、vendor binding正负2项及全仓typos1.51.1通过；原拼写失败保留，
仅对四种源码文件名下的四种完整原字符串加窄范围规则，没有排除vendor源码。
R02本次主锁匹配12→11，audit仍exit1；其他依赖与独立Octos风险未关闭。

新release二进制八组真实远程传输全部通过固定32MiB堆峰值/8MiB增量门槛，
最大Rust堆3749524字节，采样RSS31490048字节。八份原始heap总和独立核实
与报告峰值相同，输出及私有文件清理通过。证据在combined-memory-final；
保留原始profile、进程采样和源码/二进制摘要，仅移除已验证的可再生输入文件。
这些仍是隔离HTTP生产传输窗口证据，不能替代完整App、TLS或SQLite native资格。
凭据库、SDK上传总队列、Matrix链接与内置播放器继续分批处理；新提交CI尚待完成。

组合后的附件完整lifecycle7/7通过，0失败/跳过/不确定；六组真实Cargo绑定与显式文件边界均通过。

拼写证据更正：先前本地串联命令由后续diff检查掩盖了typos的exit2，
旧“全仓通过”结论撤回。83c提交的hosted拼写失败保留。实际工具源码只将
类型glob与basename匹配，完整路径规则未生效；已改原四种文件名和完整原字符串，
重新独立检查退出码。此规则也会适用于同名文件中的相同完整字符串，
不声称它具有该工具不支持的目录级匹配能力。
