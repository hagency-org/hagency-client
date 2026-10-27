# Client PR #9–#12 审核与修复（2026-10-27）

审核对象是依次堆叠的 #9 `1724b7d9`、#10 `c5f42926`、#11
`0f7a5a7c`、#12 `2daae62d`。没有合并、部署或调用付费模型。
代码与验证在独立 review worktree；没有修改其他工作的 checkout。

## 审核发现与修复

#9–#11 的改动中未发现需要修改的阻塞问题。重点检查 programming 的
配置/当前执行/精确工具批准/会计上下文，账本恢复的空库限制与原始记录，
以及文件冻结、一次性发送、未知结果与历史刷新。已设 Ready 并发布
`@codex review`。这不是合并批准，也不改变既有平台验收限制。

#12 有明确功能缺口：加密文件写入 Server 的 `encrypted_publications`，
NativeOwner 文件历史刷新却固定走普通 `file_outbox` lookup。晚到的加密
发布回执因此无法进入原文件历史。新增回归在原实现稳定失败，返回
`file_history_upgrade_required`；即使 Server 提供普通文件历史能力，
查询也不是原始加密发布记录。

修复后的刷新按账本的原始加密输入绑定路由，使用原 Agent、dispatch、
execution、request 和保留的 epoch 元数据调用只读 encrypted status。
Server status 验证当前 owner/绑定，不获取执行租约。保留原本地 efile ID
和 manifest，和执行中状态查询共用严格回执校验；错误身份、kind、状态
及缺失事件回执被拒绝。缺失/possible 发布保持未知，不能据此写 final
absence、重传或替换发送。接收 SDK 身份撤销后不提交晚到回执；显式关闭
并重开 NativeOwner 后可读取同一原回执。原 Unknown 批准审计保留。

## 验证

- 原始叠加头：Agent 单元170通过/3既有忽略，集成3+8通过；engine lib
  284通过/9既有忽略。没有把后续叠加头的测试冒充各 PR 的精确头 CI。
- 新回归真实调用 NativeOwner 和 loopback HTTP，覆盖缺失/possible、
  六类错误字段、SDK 撤销后的不提交、显式重开后的原回执读取；断言没有
  普通 files lookup、acquire、start、payload 或新发送。
- 修复前负例日志 `/tmp/hagency-pr-review-history-before.log`。
- 最终 engine lib285通过/0失败/9既有忽略；严格 hagency lib/tests
  no-deps Clippy、package format、46个静态 guards、production caller
  和 diff 检查通过。日志在 `docs/progress.md`。
- 首次扩展回归错误地尝试复用 SDK 撤销后已退休的 NativeOwner，按原安全
  约定返回 matrix_account_changed；测试改为显式重开。首次 Clippy 发现
  测试单元素循环，已简化；没有修改生产撤销规则或放宽断言。

完整 spec inventory 留给新提交 CI；本地没有安装 agent-spec，且本源码
checkout 没有 specs/project.spec.md 或 task-writer。新增 Test selector
绑定在现有 native-owner-client.spec.md。

## 保留的 CI 事实

- #9 精确头 Rust、App 双架构单元和 Windows native 通过；macOS 完整矩阵
  与 arm64 打包被取消，不能宣称这些 job 通过。
- #10 精确头 macOS 完整矩阵 Claude capacity 测试在500ms写期限下得到
  Timeout而非Capacity；App arm64被取消，Intel通过。已请求失败 job
  重跑，原失败仍保留，不通过改断言隐藏。
- #11 精确头 Rust/App/spelling通过；完整矩阵 Windows pacing fixture 的
  shutdown返回OutcomeUnknown。continue-on-error造成的 workflow success
  不等于 Windows通过。
- #12 原头 CI和后续修复头 CI按各自 SHA核对；不会把旧头成功关联到新头。

GitHub插件写操作为integration403；已使用本机已认证的gh完成授权操作。
触发Codex review只表示请求已提交，必须与服务实际完成的review区分。


## 操作者澄清后的直接复审（不使用GitHub Codex）

前述@codex只是已发布的请求，仓库没有安装GitHub Codex，不能视为外部
review。此节是直接代码复审发现，更新前述“#9–#11未发现阻塞问题”的判断。

发现P1：原生工具的本地批准消费后，还要await Server校验；另一SQLite
句柄在该等待中提交策略撤销，旧代码仍向Codex发送accept，确定性回归观察
到effect=1。106743e1在最终Server await之后重新检查原proposal的策略版本
和期限，不重复消费批准。command/file/MCP × owner/rule × 三层策略共18
组合均拒绝，既有允许执行测试仍通过。

修复以正常merge依次同步到#10（6f5551ac）、#11（0eb40cf8）和#12，
不force push、不重写历史。#10之后还断言原审计为Expired，不能进入Submitted。
各分支本地Agent测试分别96/125/154通过，均3既有忽略；最终组合另行验证。

CI结论：#9/#10的arm64取消由GitHub注释明确证明runner未获得分配、
资源不足，既无编译日志也无执行结果，不是源码失败。#10矩阵第二次三平台
均通过；原第一次Claude容量测试Timeout仍保留。#11的Windows失败在完成
HTTP断言后的SQLite关闭，原日志没有关闭阶段，不能推断具体根因。
0eb40cf8接入现有仅失败时输出的原始shutdown快照，保留原期限和panic，
不把此诊断称为Windows生产修复。macOS同selector通过；Windows需新CI观察。

本次没有新外部模型调用、安装Codex、合并PR或部署。

最终本地组合：Agent171单元+3+8集成通过，3既有忽略；engine285通过，
9既有忽略；Matrix pacing1/1；受影响库/tests严格no-deps Clippy、format、
46静态guards、production caller与diff检查通过。前一精确头6103cbba的
40509336784矩阵三平台actual jobs现均成功，fast40509323755也成功；
这些结果不冒充本轮新合并头CI。当前诊断改动未改变生产关闭行为。
