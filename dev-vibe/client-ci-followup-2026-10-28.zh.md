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
