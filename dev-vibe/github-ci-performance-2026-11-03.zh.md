# GitHub CI 与 Release 耗时审计（2026-11-03）

本次覆盖全部10个工作流及两个本地 composite action。依据是现有配置、GitHub jobs/steps 的真实时间戳、原始日志和缓存 API 快照；时间是实际执行时间，排队单列。证据原件保存在本工作树忽略目录 `.run/ci-audit/`。当前0.6.2已冻结的667e62730 / run41530365979继续独立构建，本优化不移动其源码或标签。

## 实测瓶颈

| 样本 | Engine | Installer | 主要耗时 |
|---|---:|---:|---|
| [0.6.1成功构建41390501008](https://github.com/hagency-org/hagency-client/actions/runs/41390501008) | Windows55.4分；Mac45.3分；Linux21.9分 | Intel159.8分；Windows98.4分；ARM Mac45.8分 | Engine完成后才开始Installer，约215分关键路径 |
| [0.6.2旧候选41478120359](https://github.com/hagency-org/hagency-client/actions/runs/41478120359) | Windows66.2分；Mac42.7分；Linux38.0分 | Windows100.1分；ARM Mac59.6分；Linux53.2–59.4分 | Mac Engine前五次未获得runner；第六次才真正运行。Intel打包最终因用户取消该发行目标而取消，不能算通过 |
| [普通App Build41478120572](https://github.com/hagency-org/hagency-client/actions/runs/41478120572) | — | 非安装包构建 | Windows25.3分、ARM Mac11.8分、Intel35.1分；Intel缓存上传8.1分 |
| [App CI41440209319](https://github.com/hagency-org/hagency-client/actions/runs/41440209319) | — | 非安装包构建 | ARM23.9分、Intel31.7分，含不同特性构建和测试 |
| [快速CI41478120446](https://github.com/hagency-org/hagency-client/actions/runs/41478120446) | Ubuntu28.7分 | — | 默认全套测试16.6分，另有编译与execution fixtures |

[旧候选Windows安装包job](https://github.com/hagency-org/hagency-client/actions/runs/41478120359/job/125495258391)的78分打包步骤中，主程序release编译45分、独立Octos编译29.4分；工具安装4.2分，App测试15.4分另计。[Linux amd64 job](https://github.com/hagency-org/hagency-client/actions/runs/41478120359/job/125495258360)中主程序首次20.1分、Octos13.1分，第二种格式再次主程序编译11.7分；Octos第二次仅0.62秒。不要把所有时间归因于压缩或下载，也不要把不同格式的资源编译直接当作可删除的相同操作。

ARM hosted runner容量等待属于执行之前的排队，缓存不能解决。旧候选反复等不到Mac，阻止了所有安装包启动。夜间Full matrix41515389020也因Mac未获得runner失败；Windows/Linux原套件通过。未执行的取消不视为测试通过，不调整断言或换Intel掩盖。

## 缓存：已有，但有三个路径错误

1. release设置`CARGO_TARGET_DIR=$RUNNER_TEMP/hagency-client/target`（Windows为D盘），rust-cache却仍列出仓库`target/`。原始缓存日志明确证明它保存了Cargo下载目录，却没覆盖真实编译目录。所有Installer均受影响。
2. 普通Windows App Build同样把编译移到D盘，缓存仍保存仓库`target/`。
3. portable article job缓存`crates/article-core/target`，Cargo统一workspace实际输出在根`target/`。

修复使用真实目录相对`GITHUB_WORKSPACE`的路径映射。rust-cache的target参数是相对workspace的路径，不能假设会读取`CARGO_TARGET_DIR`，也不直接传任意绝对路径。错误路径的新缓存使用新prefix，避免命中旧的仅下载缓存后不再保存编译结果。

缓存API快照返回五份条目，总计约8.57GB，单份1.54–2.38GB；usage端点同时返回3个active cache，存在统计时差，不把两个数混为一谈。缓存条目主要是Cargo下载/git目录的重复副本，Engine缓存已不在该条目列表中。接近默认容量时会发生淘汰；修正编译缓存后体积可能进一步增长，本轮不宣称已解决所有容量问题。

rust-cache通常只保留依赖编译结果，不保存workspace自身结果或测试成功状态。每次仍会编译变更代码并执行原测试。不同OS/架构/特性/profile也不能无条件共用结果。依据：[rust-cache配置与行为](https://github.com/Swatinem/rust-cache)、[GitHub缓存范围与淘汰](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching)。

## 本次改动

- Installer仅等待qualification判定，与Engine并行；公开Release仍依赖`engine + qualification + package`全部成功，严格资格/签名步骤不变。以旧候选Windows数据估算，除去排队，串行约166分可变为约100分的最大支路；这是DAG估算，非新工作流实测。Engine失败时可能浪费已并行开始的安装包计算，换取正常路径的较短等待。
- 修正上述三种编译缓存路径。Installer保留失败时缓存，但失败测试仍为失败。
- 固定版本cargo-packager0.10.1、robius0.4.1保存到独立小型安装根，按OS/架构/Rust/工具版本精确缓存。安装清单一起保留，仍由`cargo install --version --locked`核对；工具成功后立即保存，不因后续App失败丢失。依据：[Cargo安装根、版本判定及目标目录](https://doc.rust-lang.org/cargo/commands/cargo-install.html)。
- 自动Rust编译job仅在main写缓存；PR仍可恢复main缓存。PR merge-ref缓存不能反向温热main，停止它们写入可减少对主分支缓存的容量竞争。
- 去掉App Build额外的原始Cargo.lock key；rust-cache自身仍绑定工具链、配置与依赖锁，并可在依赖变动时恢复旧依赖。原key把锁hash放进restore前缀，会失去这种回退机会。
- 快速CI、App CI、App Build只排除明确的协调/发布记录及根README。原快速CI整个`docs/**`过滤过宽：里面有实际编译进程序的Agent模板。本轮收窄，模板、未知文档和配置变更继续检查；拼写检查仍覆盖文档。没有笼统跳过crates内文档、spec或qualification记录。

## 全部工作流结论

| 工作流/Action | 角色及结论 |
|---|---|
| ci.yml | 每PR的Ubuntu快速Engine门；与Full Ubuntu存在重叠，但快速反馈与真实cgroup门不同，本轮保留测试；只改缓存写入和保守文档范围 |
| engine-checks.yml | 被release/rust复用的三平台全套；全部fatal，保留原时限/选择器/Linux cgroup/Windows介质检查；不以缓存替代执行 |
| rust.yml | 夜间、手动及major PR全矩阵；scope目前额外fetch完整历史可进一步缩窄，但不是主要分钟数，本轮不改触发或nightly issue语义 |
| app-main.yml | 原生Mac双架构Clippy、Sparkle、agent_chat/octosense-module、portable检查；修正article缓存，其余验证保留 |
| app-builds.yml | 五原生编译目标，含Linux/Windows默认库、真实NSIS交接、附件内存测量；修复Windows缓存。不同feature/profile及原生集成行为不等同于Mac App CI |
| app-release.yml | 新发行范围四Installer/六安装包；改并行链与缓存；保留每平台App测试、版本/资源、签名、完整清单、草稿回读及公开回读 |
| typos.yml | 实测约16秒，已有并发取消；不能用删除拼写门来解决小时级release问题 |
| windows-probe.yml | 手动/特定probe分支诊断，非普通PR重复门；诊断不替代原失败，不改 |
| app-licenses.yml | 手动生成许可证，cargo-about已有独立缓存；不增加自动触发、不动其写权限 |
| app-aur-publish.yml | 手动AUR发行，容器原生构建属发行验收；不合并进自动Client发布，不触碰凭据 |
| import-apple-cert | 仅配置签名才执行；保留证书/钥匙串边界 |
| setup-cargo-makepad | 手动移动端才使用，按锁修复独立构建；并非日常release主耗时，不改 |

当前main API返回protected=false。本轮不改仓库权限/保护设置；PR合并仍以实际检查结果与复核为准。Full matrix、App CI和App Build都已有并发取消，release故意不取消正在发行的同ref任务；不为了速度把它改成自动中断发行。

## 保留的优化空间与验证边界

- Octos是独立固定源、固定锁、独立target的release编译；仍花13–29分钟。应在容量预算明确后再设计单独的依赖缓存或固定构建产物验证。当前根target的通用清理不适合直接保存它的源工作树；不以未验证的本地Octos或历史可执行文件替换。
- Linux每种格式有不同Makepad资源参数；要消除第二次编译，需验证最终ELF、运行时资源路径及两格式实际启动，不能仅删beforeEach hook。本轮没有修改打包hook。
- 可进一步合并同环境Ubuntu快速/Full检查，或共享依赖缓存；但须保持现有check身份和独立宿主验收。不能直接复用另一个SHA的成功状态，也不能把all-feature no-run误当已执行默认全套。
- 修正后缓存可能超过仓库限额：后续用命中率、条目尺寸和压缩/上传时间选择共享下载缓存、远端编译缓存或经授权扩容。没有擅自购买runner、扩大额度或安装外部缓存服务。
- 本地已通过修改的五个workflow actionlint（含ShellCheck）、实际执行release目录准备脚本并确认缓存目标等于Cargo输出，Windows步骤PowerShell解析、跨盘布局同盘相对路径核对、diff检查。全仓actionlint只有未改AUR原SC2129 style提示；不伪称原仓无提示。
- 合并前需复核PR差异、真实原生检查和非发布Installer流水线；保留任何原始失败。未完成hosted结果、冷/热缓存测速或新安装包启动之前，不给出已提速百分比或平台资格通过的结论。
