# R02：mdurl 的旧 IDNA 依赖修复

本批只修主 `Cargo.lock` 中 `mdurl 0.3.1 → idna 0.3.0` 这一路径。
基线是 `361d4e34ae5be9cb530b6418d023c50fba02c1ee`；实现位于隔离的
`codex/quality-dependencies` 分支。本地实现和验证完成，尚未推送或创建 PR，
也未取得本批 hosted CI 结果。依据是已接受的
[REQ-CLIENT-QUALITY-REMEDIATION](../knowledge/requirements/req-client-quality-remediation.md)
和[本批任务合同](../specs/task-client-idna-dependency.spec.md)。

`vendor/mdurl` 来自实际 registry 原始包，SHA-256 为
`5736ba45bbac8f7ccc99a897f88ce85e508a18baec973a040f2514e6cdbff0d2`，
对应上游 commit `0a7536e3ffc7f5688d3b06ab647ebcfbc511d1cb`。
完整 MIT 许可和四组原始归属、README、源文件及原始测试全部保留。
原始归档文件的摘要记在 `UPSTREAM-SHA256.json`；有效的 `Cargo.toml`
将内部 IDNA 要求精确改为 `=1.1.0`。父复核另发现原生构建的
`RUSTFLAGS=-D warnings` 会拒绝两处原有 lifetime syntax warning，因此
仅在 encode/decode 两个返回类型补 `Cow<'_, str>`，
保持原推导生命周期和行为，没有加 lint 豁免。
版本仍是 0.3.1，这不是新的上游发布；新增测试和补丁说明单独可见。
root patch 指向该副本，workspace 显式排除它；registry/Git 源缓存未被手工修改。

合并候选的实际 typos 1.51.1 首次检查标记了上游原样 IPv6、截断域名向量、
一个测试标识符和原始注释；没有改写这些来源字节。`typos.toml` 只对对应四种源码文件名忽略四种完整原字符串（工具只匹配basename），其余内容继续检查；原失败日志保留。

实际 Matrix formatter 只调用 mdurl 的 percent encode/decode。
`links.rs` 的主机名转换继续使用既有 `idna 1.1.0::punycode` 原语；
fullwidth、相对路径、Matrix URI 和已转义字节的 JS 对照契约不变。
mdurl 内部两个域名格式化 API 的签名保持兼容；旧依赖的 ASCII-only/空 ACE
别名折叠则按新版 IDNA 规则修复。此结果不等于批准新的 IDN 显示或域名鉴权策略。

## 实际验证

- 原包 133 项测试全部通过：46 unit（format 24、Url 1、percent 21）和
  87 parser integration。之前只统计 percent/parser 的 108 项是漏计，已纠正。
  新增两公开 API 的 4 项安全/兼容回归、4 项原始 doctest 也全部通过；共 141 项。
- 实际未修改的 registry 源码独立副本，携带同一新安全回归作负对照：
  `https://xn--example-.org/` 被旧依赖改写为 `https://example.org/`，
  测试如期失败；当前副本保留原始 malformed host，4 项新回归通过。
  旧版本失败日志保留，没有改旧源码或放宽断言。
- `hagency-matrix-format` 全包 6 项通过，其中真实 formatter 对照保留的
  63 个 JS 派生向量逐字节相等。7 个既有 terminal-display parity 测试仍 ignored，
  不计通过；零 doctest 不计通过。本 Client 副本没有旧 JS 生成器/运行源码，
  本批没有声称重新执行 JS；固定语料、其来源摘要和 Rust 实现均未改变。
- 自身 formatter 的 all-targets、no-deps 严格 Clippy 通过。父复核实际 native warning policy 的首次失败保留；两处纯语法
  lifetime 注释后须重新通过相同 policy，不将之前 own Clippy 代替整依赖构建。
- root `cargo test --locked -p mdurl` 实际选择了这个 excluded path dependency。
  真实 metadata 证明唯一 mdurl 指向 vendor 且不在 workspace members；
  实际 inventory 含 137 项测试名。新增 Node integration 正/负 2 项通过：
  当前契约的 4 个新 selector 与 App 的 6 个 selector 均可解析，移除真实的
  4 个 vendor selector 后，契约明确报告 missing。
- Cargo 拒绝对外部 package 使用 `--all-features` 的首次 inventory 尝试，
  原 101 结果保留。因此现有 engine all-feature inventory 命令保持原样，
  另执行实际 mdurl 默认配置的 `--list`，没有 selector allowlist 或成员伪装。
  fast/full CI 都明确执行完整 mdurl 测试和该正负 integration。
- 原 binding Node 测试 19/19、actionlint、源码摘要比对通过。
  agent-spec 1.4 的最终 parse/contract 和 lifecycle 通过：文件边界及两组
  实际 Cargo 绑定共 3 pass、0 fail/skip/uncertain，lint 保留 1 条非执行路径
  不变约束的 coverage 提示。独立 Node/audit 结果分别记录。完整 engine
  all-feature inventory 未在本批本地全量重跑；其命令保持原样，交由当前提交 CI。

## 审计范围与剩余问题

两次 fresh `cargo audit --json` 使用同一 advisory DB commit
`ee1a49a2769a24a596afc1c20cb33748d2769586`（1386 条记录）。
主锁漏洞匹配从 12 条降到 11 条，唯一移除的是
`idna 0.3.0 / RUSTSEC-2024-0421`，没有新增匹配。两次 audit 都仍返回 1，
没有忽略、豁免或把它记成全审计通过。warning 数也保持
unmaintained 13、unsound 18、yanked 4。

| 仍有匹配的包 | 版本 | Advisory |
| --- | --- | --- |
| crypto-bigint | 0.5.5 | RUSTSEC-2026-0387 |
| lopdf | 0.36.0 | RUSTSEC-2026-0187 |
| quick-xml | 0.37.5 | RUSTSEC-2026-0194、2026-0195 |
| rsa | 0.9.10 | RUSTSEC-2023-0071 |
| zip | 2.4.2 | RUSTSEC-2026-0367、2026-0368、2026-0369 |
| zstd | 0.13.3 | RUSTSEC-2026-0391 |
| zstd-safe | 7.3.0 | RUSTSEC-2026-0398、2026-0403 |

独立 Octos 锁没有在本批迁移或重新审计，旧报告的数量不是本次新鲜结果。
其他依赖安全项保持开放。本次没有运行真实 Agent、模型或修改任何账号。
审计事实和实际域名调用边界分别评估，不能从匹配消失推导全部依赖或整个产品安全。

原始日志位于本工作树 `.run/quality-dependencies/`，包括 fresh 前后 JSON、
旧依赖负对照、完整原测试、新回归、App corpus、strict、实际 graph/inventory
正负证明及 agent-spec 生命周期。发布/合并仍需独立复核和当前提交 CI。

父复核补充：保留原生 `RUSTFLAGS=-D warnings` 的实际首次失败，两处显式 lifetime 注释后同一 formatter 依赖构建通过；重新运行完整 mdurl137+doc4、formatter6/7原忽略及 lifecycle3/0/0/0 通过。父评审独立核对原始 archive SHA，并逐个比对19原文件，变化严格限于 manifest 依赖与两行等价 lifetime；未加 lint 豁免或改变percent行为。


## Hosted 布局门禁复核

a30提交的macOS ARM精确特性App810项及严格Clippy通过，随后布局脚本把已
明确排除的`vendor/mdurl`误判为自有crate而失败；原日志完整保留。布局检查
现在只接受Root registry patch中明确登记、原路径exact exclude、位于vendor、
不含路径/目录symlink别名、manifest非symlink且包名匹配、非workspace member的
那个manifest，其他越界和嵌套workspace仍拒绝。没有排除vendor整树或忽略失败。

独立复核用真实Git/Cargo小工作区检出首次修正仍接受目录别名的问题：先resolve
再核对exclude会把别名混同为已登记路径。最终先核对原路径和全部目录身份，
再登记manifest，保留该负例。实际Node集成4/4通过：原真实metadata/inventory
正负2项；完整主工作区正例及两个未登记/嵌套假crate拒绝；真实registry patch
最小工作区control通过，Cargo实际接受目录别名后，检查器分别拒绝未exclude
别名和已exclude但仍为symlink/junction的别名。Windows使用junction，
路径输出统一posix，未声称本机测试取得Windows执行资格。

布局脚本确认31个workspace package及唯一显式vendor patch。R02完整绑定
lifecycle3/3通过、0失败/跳过/不确定；运行时代码和锁文件未改变，不把之前
App成功等同为新提交平台CI通过。最终三文件独立复核无已知阻断；新head仍需CI。
