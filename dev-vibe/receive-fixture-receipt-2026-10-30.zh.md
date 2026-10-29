# Windows receive 夹具：保留原始 writer 收据

本批基于 `361d4e34ae5be9cb530b6418d023c50fba02c1ee`，位于独立
`codex/quality-execution-fixtures` 分支，未修改 primary 或 R02 分支。
依据[已接受质量要求](../knowledge/requirements/req-client-quality-remediation.md)
和[本批合同](../specs/task-receive-fixture-receipt.spec.md)。

原 PR19 Windows job `123652027082` 的 `owned` binary 为 50 pass / 1 fail：
`receive::native_receive_workspace_authority_retirement` 在 grant helper 的
`start_received_file_write().await.unwrap()` 收到 `OutcomeUnknown`。
失败发生于接收 grant 的准备，不是退休后的拒绝或物理字节断言。
原始完整日志按字节保留在本工作树 `.run/quality-execution-fixtures/`；
源路径、原 head、长度及 SHA-256 记录在 `original-evidence.json`。
没有清理 secondary 既有 `.run` 证据。

## 根因范围

源码存在两种不同的 unknown：

- `DomainStore::call_with_policy` 的原始两秒回复观察界；已开始的 writer
  可在观察者超时后继续实际 IO，不能据此断言成功、回滚或重试。
- `received_files::begin` 对已经不处于 Reserved 的原 reservation 返回 unknown，
  避免再次取得不可克隆的原写入 grant。

该 execution case 对四个不同 event 各 reserve/start 一次，没有故意重放。
但旧日志没有 event index、row state 或 writer 内部计时，不能由错误文本
复原当时具体排队/IO阶段，更不能声称已捕获 Windows 磁盘耗时。
可确认的问题是夹具把有界生产 RPC 当成“必定取得准备收据”的接口：
它需要原始 grant 去验证 workspace，生产 RPC 却正确保留 unknown 结果。

不能用第二个 `DomainRepository` 修补：真实 domain.lock 拒绝双 owner。
也不能提前手工设 Started、恢复丢失 grant 或重试非幂等 begin。

## 窄修

`test-fixture` 是显式默认关闭 feature。仅该 feature 编译
`fixture_receive_write_once`：它把 typed setup 入队到同一个原 writer，
在实际 SQLite IMMEDIATE 事务中采样 writer clock，复用真实 reserve/begin，
校验当前 Started/capability/scope/event/limit/facts，再把一个原始
`ReceiveWrite` 经原 oneshot 返回。队列和八 MiB 入参预算仍保留。
不存在一般性的公开可变 DB callback。

这个测试入口等待原收据；execution helper 有独立十秒 watchdog。
超时仍使测试失败，不恢复、重试或宣告原事务成功。普通库 compiler artifact
实际记录的 feature 不含 `test-fixture`；Store/Execution 的测试仅通过
各自 dev-dependency 显式启用它。生产 `call_with_policy` 两秒界及 unknown
处理字节未改，普通 reserve/start API、schema 和授权状态转换未改。

原 execution helper 仍在真实离线 child 的 StartedWorkspace 之后准备 grant；
只替换 setup 收据观察方式，并补原 cap/facts 相符断言。所有原测试中的
坏 capability、过期写入、真实 cancel、未创建文件、旧文件内容和退休拒绝
断言保留；四秒 workspace 写入/读取窗口也保留。

## 实际证据

- 旧 `native_domain_unknown_reports_dequeue` 和
  `native_receive_original_write_once` 两个现有回归分别通过，确认两种语义。
- 新增两项受控回归通过。线程局部 SQLite statement trace 在真实 begin 的
  UPDATE 前门闩阻塞原 writer，使用真实生产 timer，没有假时钟或 sleep 重试。
  实际公开 production start 超时返回 Unknown，没有 grant；原操作释放后
  仅观察到 WritePossible，未把它当作文件写入成功，也未重试。
- fixture 的原 begin 同样被门闩阻塞；其后的真实生产只读 RPC 两秒后仍
  返回 Unknown，fixture 此时仍等待原 receiver。门闩释放后才得到原 grant，
  cap/facts 正确，真实 received row 数为一。unit 的 Started 是明确的域数据
  fixture，不声称物理 child；下一项 execution 回归提供真实 child 证据。
- 原 execution `receive::` 六项全部通过，包括原失败 case；这些 case
  使用实际离线 probe Started、原 store 和真实私有文件，不运行模型。
- 普通 Store lib check 通过并证明 feature 关闭；Store/Execution 自身
  all-targets strict Clippy 和 fmt 通过。合同 parse/contract 通过，lifecycle
  为边界和两组实际绑定共 3 pass / 0 fail、skip、uncertain；lint 保留一条
  file-output 模式启发式提示。完整原始结果保留，不把未运行目标记为通过。

本批本地平台是 macOS，未把旧 Windows 失败标为已实机复测通过。
新提交仍需独立复核和当前 Windows CI。没有扩大生产时限、允许失败、
忽略用例或修改真实账号；物理 Windows 延迟的具体来源仍是未观测限制。
