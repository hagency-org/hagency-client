# Hagency Client 质量改进执行记录

本记录对应[完整评审](client-quality-review-2026-10-29.zh.md)。用户已授权分批修复、提交 PR，并在代码复核和当前版本 CI 均通过后合并。整体目标仍有效；本页是协调与证据记录，不替代运行时任务状态。

第一批从 `origin/main` 的 `932dcdea` 开始，位于独立工作树；未混入已有的 tutorial PR 或原工作区未提交材料。Task Contract 为 [task-client-quality-hardening.spec.md](../crates/hagency-app/specs/task-client-quality-hardening.spec.md)。

## 完整改进清单

| 编号 | 目前状态 | 交付与剩余工作 |
| --- | --- | --- |
| R01 图片请求内网边界 | 第一批实现并回归通过 | 每跳审核全部 DNS 地址并绑定连接，关闭代理和自动跳转；mapped/特殊地址拒绝，保留流量和时间预算 |
| R02 双锁依赖与审计 | 部分修复，继续 | 主锁兼容更新后匹配21→12；独立 Octos 的47条仍需真实发行 feature 分流及更新；持续安全门禁待实现 |
| R03 普通附件下载预算 | 第三批实现与测量中 | 私有文件流式传输、4操作/16排队、实际字节/磁盘额度、取消、TTL、加密完整性；最终峰值与当前SDK平台CI仍待通过 |
| R04 发布资格闭环 | 第二批实现，验证中 | 所有桌面引擎必须成功，公开发布前检查同源码真实资格；当前占位记录仍正确拒绝发布 |
| R05 系统凭证库 | 待实现 | token/密钥迁移、不可用及中断恢复、Windows DACL |
| R06 分享明文生命周期 | 第二批实现，验证中 | 私有句柄、30分钟TTL、预算、退出及崩溃回收；原生分享无完成回调，已交付外部副本无法撤销 |
| R07 流式上传 | App侧边界实现中，SDK队列仍未解决 | 实际文件身份/大小/修改时间、64MiB单源/128MiB准备预算；SDK接手后及恢复队列的总预算/流式上传仍开放 |
| R08 下载完成锁与阻塞 | 第二批实现，验证中 | 回调前释放锁；有限后台准备、写盘、门户响应观察及账号失效围栏 |
| R09 资料缓存 | 第一批实现，统一PR平台验证中 | 用户/成员/完成队列有界，合并与有限UI批次，容量拒绝延期重试 |
| R10 历史索引公平性 | 第二批实现，验证中 | 两个有限page worker、FIFO轮转、精确房间锁、原有cursor事务；已进入SQLite事务不能撤销 |
| R11 快速建卡草稿 | 第一批实现，统一PR平台验证中 | 按Board保留原文与未知写幂等键、32个草稿显式容量、取消/确认保存/换账号清理 |
| R12 全局审批发现 | 第二批实现，平台验证中 | 常驻入口、账号级有界分页与badge；只观察现有owner，精确路由后重新核对请求；未实现系统通知 |
| R13 原生永久链接 | 待实现 | matrix.to/matrix:、alias/via、事件上下文定位和权限反馈 |
| R14 音视频操作 | 原有操作已核实，第三批文案和点击回归中 | 原时间线及历史已有下载/分享/另存为；修正音视频指引并验证真实控件；内置播放器仍未实现 |
| R15 加密能力声明 | 第一批文档与UI已更新 | 中英README/quickstart/privacy/release notes区分owner与shared/恢复及资格边界 |
| R16 契约与模块热点 | 协议错误已修，结构持续改进 | discovery校验与返回均为3；在后续传输和交互修复中收敛职责 |
| R17 worktree重复编译 | 第二批实现，平台验证中 | 只监视真实Git HEAD/ref/packed路径及linked指针，不为源码archive虚构路径 |

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
