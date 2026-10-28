# Client PR CI 跟进（2026-10-28）

检查对象：#9 `106743e1`、#10 `6f5551ac`、#11 `0eb40cf8`、#12 `321f0d06`。
快速CI均通过。#9三平台完整矩阵及App通过；arm64打包未获得runner。
#12 App的Intel通过、arm64未获得runner；这些是明确GitHub容量注释，不是
编译或测试失败。只重跑相应失败job，保留原始结果。

#12完整矩阵40518755948的两个实际失败：

1. macOS Palpo publication rotation在预期Generation处得到Timeout。
   该正向场景会在收到旧HTTP请求后，先提交替换generation和publication，
   再放行旧响应，但复用了专用于短deadline负例的300ms headers预算。
   保留旧响应400ms的受控实验在原配置稳定复现相同失败。此场景改用
   现有生产默认的有限5s headers/10s request预算，并保留受控延迟与所有
   Generation、原始字节、重启及新sequence断言。专门HTTP deadline负例
   的300/800ms预算不变，生产HTTP代码不变。历史机器上慢的具体系统调用
   无日志可证，不宣称定位到某个SQLite或调度调用。
2. Windows long-owned turn在17.92s因LeaseRenew/TimedOut结束，pending RPC0，
   原进程树已停。源码映射证明是DomainStore2s回复界限，不能证明是OS
   磁盘或调度。保留2s界限、5s续租与45s操作预算；CI将这个真实31s正向
   场景作为必须通过的exact selector单独执行一次，然后仅在后续并发调用
   中排除它。失败直接结束step，不能通过第二次运行覆盖。其余并发场景及
   测试内并发保持原样。补充仅诊断的last-unknown/dequeued文本。

本地Palpo16/16（含未改deadline负例）、owned52/52、严格受影响all-target
no-deps Clippy、format、actionlint、bash语法和三平台工作流路由检查通过。
受控旧配置失败与修复日志见docs/progress.md。新提交CI必须另外验证，
原失败不重分类；最终远端实际job结果以对应PR的跟进评论记录。

## arm64基础设施修复

官方[runner-images公告13518](https://github.com/actions/runner-images/issues/13518)
明确macOS14正在退役，2026-11-02停止支持，期间有容量限制及brownout。
将app-main和app-builds的arm64及可选移动端host统一迁到固定macos-15，
继续保留macos-15-intel及双架构测试/构建。产品最低部署目标12.0不变。
7ea40644从#9正常merge同步到整个堆叠链；actionlint通过。此前没有获得
runner的job不能算通过，新镜像必须以新SHA实际执行验证。

## 全堆叠同步

Palpo/execution生产源和owned harness在四个分支一致，因此同样的fixture
修复也通过d53d31e6下沉到#9，再正常merge到#10 ffabbc21、#11 57058917
和#12 3a3c2f7f。基础分支Palpo16/16、owned52/52、严格all-target Clippy
再次通过。没有force push；后续只检查这些新头及包含本记录的#12最终头。

## 纯HTTP fixture的无关数据库依赖

新头d53d31e6的完整矩阵40561752670中，Windows job122393152490的唯一
实际失败是native_matrix_get_rate_limit末尾DomainStore::shutdown返回
OutcomeUnknown（rate_limit_tests.rs:64）。四轮HTTP响应、重试等待、凭证和
请求计数断言已全部通过；不能把它误报为HTTP Timeout，也不能证明是哪段
OS/数据库关闭阶段超时。原始完整日志保留在对应Actions job。

rate-limit与pacing两个纯HTTP fixture仅为HostConfig启动完整domain仓库和
writer，测试过程从不访问它。改为直接调用Http::new原本委托的
Http::for_host，保留相同TLS根、凭证、endpoint、common limits及每场景
预算，不再创建无关writer。所有HTTP断言、真实TLS peer与fake.close保留。
生产代码及数据库关闭2s界限完全不变；专门store和service生命周期测试
继续保留真实writer，任何关闭错误仍失败。旧pacing关闭观察记录为历史。

本地Matrix库280/280、HTTP全部12项、数据库关闭8/8、严格Matrix all-target
no-deps Clippy、format及静态guard46/46通过。修复从#9 fe449853经正常
merge631a342f(#10)、29b20c1d(#11)传到#12。新头CI须另外实际完成。
此前9db9212f的Windows完整job已实际通过（含31s owned与媒体验证），
d53d31e6的双架构App CI和全部桌面打包也通过；这些不是新头CI的替代。

## #12 Linux执行器fixture资源隔离

8e0e50fd的Full matrix40572803517/Linux job122431362644失败于四个
execution用例。untransmitted的owner-verdict返回OutcomeUnknown；长生命周期
控制在12.54s因LeaseRenew/TimedOut退出，last_unknown_dequeued=Some(true)，
原进程树已完整停止。usage-control只记录到通知流提前关闭；midwrite在
2.5s harness等待后报告marker已出现在诊断列表中。这能证明两个数据库
回复时限失败及两个fixture等待失败，不能证明未记录的OS阶段，也不能把
另外两项症状直接归因为相同原因。HTTP库测试均通过。

本地库42项在16线程和1线程下均通过，未复现云端全部症状。#12将execution
所有目标放入独立的单测试线程CI阶段，其他包仍按原并发执行；单个测试内
的并发、fault gates、实际child/SQLite、2s数据库回复、25/45s操作预算和
全部原断言不变。后续workspace仅移除execution包选择，保持每个普通用例
正好执行一次；原ADR140/144过滤不变，独立阶段失败仍致job失败。先前只
隔离Windows长用例的策略由这里的全平台execution阶段取代。另补充两个
失败路径的原始operation停止/cleanup和unknown/dequeued文本，无重试。

第一次本地全目标命令漏带已有qualification过滤，正确触发两个真实Codex
资格证据placeholder失败（108项普通用例通过），该原始失败不改判。按
CI准确过滤重验；两项资格门禁未运行，不声称获得真实sandbox资格。
本轮仅更新#12，其他PR最新头继续原CI；最终新头结果另以PR评论记录。

准确CI命令重验结果：execution普通测试108/108通过，2项原qualification
过滤、0 ignored；严格all-target no-deps Clippy、fmt、actionlint及解析
YAML后的包集合/正好一次/原过滤/失败致job失败规则核对全部通过。

## 生产store成功ACK的锁释放缺陷

ccda4501快速CI40578195066/job122450182413在factory重启失败：原domain
shutdown已返回成功，紧接着DomainRepository::open却返回Locked。旧错误
映射丢弃了原生锁错误，因此历史errno或持有描述符的具体child没有被记录。

实际受控回归在两种store都保留原ownership File的try_clone，再调用普通
shutdown：旧实现稳定返回成功而立即重开Locked；修复后普通/observed两条
路径都立即重开成功，并验证旧副本稍后关闭不会解锁仍独占的新writer。
这是实际内核文件锁和原writer/数据库的回归，没有伪造writer结果。
[Rust File锁契约](https://doc.rust-lang.org/std/fs/struct.File.html#method.unlock)
说明副本/继承描述符仍活着时，仅关闭原句柄不足以释放锁。客户端Unix
pre-exec只在exec时关闭CLOEXEC描述符；并发spawn有这样的保留窗口。这解释
一个可真实复现的缺陷机制，不证明历史CI中是哪一个child或原生错误。

两个worker现都在SQLite连接已关闭后，显式unlock原ownership并关闭原句柄，
然后才允许成功ACK。解锁错误记录后关闭reply，仍为Unavailable/ReplyClosed
或原超时，不制造成功；receiver-before-ACK、2s界限、NO_CKPT_ON_CLOSE、
WAL/FULL、隐私文件策略及schema均不变。try_lock只有真正WouldBlock归类
Locked，其余保留原生IO错误。原factory重开代码没有增加重试或等待。

本地store库105/105、WAL重开1/1、inline_factory21/21（含原失败重启）、
execution普通用例108/108（原qualification过滤2）、严格store/engine/
execution all-target no-deps Clippy、fmt、guard46/46、生产调用边界和Rust
全功能绑定1167项/0 missing全部通过。两个回归在旧代码失败日志保留于
/tmp/hagency-ci3-inherited-lock-before.log，修复日志为inherited-lock-after、
store-release-final、ownership-inline、ownership-execution、ownership-final-
clippy、ownership-bindings等同前缀文件。

共享生产修复从#9 8bb16bd6正常merge到#10 07df6724、#11 cd57377b和#12。
#12保留其execution独立CI阶段与原始失败诊断。所有新头重新实际验证，最终
证据记录在各PR的跟进评论，避免通过后再创建仅含记录的新SHA。

## 重启scenario的平台声明修正

8bb16bd6的Windows job122474169790在绑定gate失败：新store contract漏了
原native_configured_fleet_reattaches_after_restart既有的Linux/macOS作用域。
只给这个scenario补上Test-Platforms，与原factory spec和源码cfg一致；
两项新ownership回归仍为跨平台必需项。没有改Rust或新增运行时skip。
本地用同一绑定器复现Windows缺失，再验证Windows必须绑定2项、只defer原
重启1项，Linux/macOS3项全部绑定；guard46和实际Mac全功能库存再验通过。
该修正从#9 c8bd20b7正常merge到#10 0498473e、#11 353036ec及#12；
最新SHA重新CI，旧Windows绑定失败保留，不当作实际锁测试失败或通过。


## 2026-10-28 — App CI 整体 job 预算

#12 当前头42484608的App Intel job122486397191全部测试、Clippy、构建和静态检查均成功，最后在59m20s开始缓存上传，60分钟整体job上限将其取消；GitHub annotation明确 exceeded maximum execution time 1h0m0s。原attempt1保留，单job诊断补跑已触发。为避免依赖冷构建速度碰运气，#12 app-main的整体job预算从60改为有限90分钟，包含缓存和cleanup；原用例、断言、生产2s/25s/45s边界及执行次数完全不变。此为编译和收尾总预算，未放宽任何测试操作deadline。新头需重新通过实际CI。
