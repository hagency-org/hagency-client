# Hagency Client 质量改进执行记录

本记录对应[完整评审](client-quality-review-2026-10-29.zh.md)。用户已授权分批修复、提交 PR，并在代码复核和当前版本 CI 均通过后合并。整体目标仍有效；本页是协调与证据记录，不替代运行时任务状态。

第一批从 `origin/main` 的 `932dcdea` 开始，位于独立工作树；未混入已有的 tutorial PR 或原工作区未提交材料。Task Contract 为 [task-client-quality-hardening.spec.md](../crates/hagency-app/specs/task-client-quality-hardening.spec.md)。

## 完整改进清单

| 编号 | 目前状态 | 交付与剩余工作 |
| --- | --- | --- |
| R01 图片请求内网边界 | 第一批实现并回归通过 | 每跳审核全部 DNS 地址并绑定连接，关闭代理和自动跳转；mapped/特殊地址拒绝，保留流量和时间预算 |
| R02 双锁依赖与审计 | 部分修复，继续 | 主锁兼容更新后匹配21→12；独立 Octos 的47条仍需真实发行 feature 分流及更新；持续安全门禁待实现 |
| R03 普通附件下载预算 | 待实现 | 私有文件流式传输、全局并发/排队/实际体积与磁盘额度、取消及加密校验 |
| R04 发布资格闭环 | 第二批实现，验证中 | 所有桌面引擎必须成功，公开发布前检查同源码真实资格；当前占位记录仍正确拒绝发布 |
| R05 系统凭证库 | 待实现 | token/密钥迁移、不可用及中断恢复、Windows DACL |
| R06 分享明文生命周期 | 第二批实现，验证中 | 私有句柄、30分钟TTL、预算、退出及崩溃回收；原生分享无完成回调，已交付外部副本无法撤销 |
| R07 流式上传 | 待实现 | 实际文件句柄和字节边界，避免先完整缓冲再拒绝 |
| R08 下载完成锁与阻塞 | 第二批实现，验证中 | 回调前释放锁；有限后台准备、写盘、门户响应观察及账号失效围栏 |
| R09 资料缓存 | 第一批实现，等待PR与CI | 用户/成员/完成队列有界，合并与有限UI批次，容量拒绝延期重试 |
| R10 历史索引公平性 | 第二批实现，验证中 | 两个有限page worker、FIFO轮转、精确房间锁、原有cursor事务；已进入SQLite事务不能撤销 |
| R11 快速建卡草稿 | 第一批实现，等待PR与CI | 按Board保留原文与未知写幂等键、32个草稿显式容量、取消/确认保存/换账号清理 |
| R12 全局审批发现 | 第二批实现，UI复核中 | 常驻入口、账号级有界分页与badge；只观察现有owner，精确路由后重新核对请求；未实现系统通知 |
| R13 原生永久链接 | 待实现 | matrix.to/matrix:、alias/via、事件上下文定位和权限反馈 |
| R14 音视频操作 | 待实现 | 明确下载/打开操作，准确文案，分阶段播放器及生命周期 |
| R15 加密能力声明 | 第一批文档与UI已更新 | 中英README/quickstart/privacy/release notes区分owner与shared/恢复及资格边界 |
| R16 契约与模块热点 | 协议错误已修，结构持续改进 | discovery校验与返回均为3；在后续传输和交互修复中收敛职责 |

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
