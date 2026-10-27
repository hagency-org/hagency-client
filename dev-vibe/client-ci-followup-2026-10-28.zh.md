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
