# 主人原文件投递历史与容量结清

## 当前实现

“我的Agent → 文件投递”提供分页列表、原记录详情和“刷新投递回执”。详情包含原
文件声明/摘要、Room/请求者/线程、执行与请求编号、实际Matrix事件和审批记录编号。
切换Agent或账号会清除旧选择；过期异步响应不能填回另一Agent的详情。未知、排队、
上传和实际送达分开显示；刷新失败保留原状态。中英文案一致，窄窗口按钮可换行。

NativeOwner的Files/File/RefreshFile动作仅在当前SDK账号、Server/issuer/subject/owner
和设备身份内读取原账本。刷新要求agent-file-history-v1，使用原dispatch/execution/
request编号，不依赖原file_id或lease，也不启动模型或重新发送。远端响应严格核验
原声明；读取完成后再次核对SDK账号与设备，才允许写入本地观察。旧Server明确要求
升级，网络错误或普通404都不能构造取消回执。

## 三类事实及恢复

1. 有原delivery：仅接受原ID/声明、字节数、上传状态和状态机不倒退的回执。只有
   sent及实际事件证明送达，cancelled结清原文件容量；旧unknown审批不会因迟到
   sent而改写为完成。
2. 无delivery且preparationClosed=false：只是暂时缺失，保留未知及全部容量。
3. 无delivery且preparationClosed=true：Server在与prepare/终态相同的事务锁内确认
   原执行永久不能再准备此文件。新增sent_file_absence永久保存此证据及观察时间，
   释放文件容量，但不伪造sent/cancelled、不删除发送见证/审批，也不恢复原写权。

SQLite约束校验缺失证明的原请求和无delivery/关闭条件；删除/修改证据，以及在
最终缺失后追加冲突delivery，均拒绝。原记录的精确身份始终不能重发。新增不同
原文件仍需要新的真实字节和审批；容量释放不等同于模型Token预留结清。

当前及此前发送Schema的快照均保留原记录；旧快照只添加空证据表，不推断历史。
恢复先插入原发送见证，再插入最终缺失证据，保留校验触发器；没有用禁用触发器
绕过一致性检查。重开/恢复不能构造Custody。

## 验证范围

- 真实磁盘/SQLite覆盖暂时与最终缺失、不同owner/Agent/原请求拒绝、永久证据、
  重开/快照/旧Schema、稳定分页及原身份不能重发。实际16份8MiB预留达到128MiB，
  暂时缺失仍拒绝新文件；最终缺失后允许新的已批准文件，但原行和证据仍保留。
- 当前NativeOwner实际OAuth/设备/TCP/账本路径覆盖结束执行后的历史列表、升级拒绝、
  prepare响应丢失时取回原记录、未知→实际sent、最终缺失及查询途中SDK失效。
  失效后不存证明、不释放容量；所有文件HTTP操作都是lookup，没有acquire/start/
  prepare/chunk/queue。provider/Server回执明确是合成fixture，不代表真实Matrix联验。
- Makepad实际组件/脚本实例覆盖文件页显示与切换、未知/实际回执、旧Agent/旧选择
  回包拒绝、busy控制和切换Agent清空。这里只证明组件行为；桌面人工联验仍待完成。

日志：`/tmp/hagency-file-history-local-fixed.log`（142/原3忽略）、
`/tmp/hagency-file-history-native-fixed.log`、`/tmp/hagency-file-history-ui-verified.log`，
以及收尾all-libraries/clippy/guards/callers/fmt/spelling/bindings日志。

原失败保留：local-first拒绝了仍残留新表的伪“发送前Schema”fixture；修正fixture
移除未来表后继续严格校验。local暴露快照恢复按表名排序与新INSERT校验冲突，现
只调整依赖表恢复顺序，未关闭校验。engine-first/原native/UI构建日志保留新增字段、
测试参数和Cx借用编译错误；native缺少原模型预留导致真实审批拒绝，现补齐测试
的原预留而未更改授权规则。UI组件测试还发现不存在的Title模板；现使用现有Text
模板，保留脚本错误断言与原失败日志，未跳过组件实例化。

## 依赖及剩余门槛

Server依赖PR7的0a1ef55，其CI40406382037已通过，107项PG/HTTP/AS回归和契约检查
通过。Client fe000bb7的CI/App/矩阵仍属上一提交的验证，不能代表本批新界面/Schema。
本批推送后另行运行。真正当前Client+Server+Matrix+真实模型文件路径、桌面人工、
Windows文件工具和跨平台门槛仍在总计划内；A4加密执行/文件/远程审批、A3多Agent
协作及此前阶段剩余资格继续实施，A2与总体目标均未宣告完成。

最终同一源代码完整回归：Agent142/原3忽略、引擎261/原7忽略、App620/原2忽略，
均0失败；programming3和runtime8通过。严格Agent/引擎/App/miniapp all-targets
no-deps Clippy（含原固定Sparkle SDK）通过，核心格式、46项守卫、1项production
caller通过。最终规格1160项，missing/deferred/platformDeferred为空；原superseded
仍保留。五项新Unix场景显式绑定linux/macos，Windows未被改写为已验证。
首次规格清单拒绝了放在Test之后的平台标注；现按既有格式放在Test之前，未修改
校验器或放宽平台。原bindings日志保留，最终为bindings-final.log。

最终日志：`/tmp/hagency-file-history-all-libraries.log`、
`/tmp/hagency-file-history-programming-runtime.log`、`/tmp/hagency-file-history-clippy.log`、
`/tmp/hagency-file-history-{guards-final,callers-final,bindings-final,fmt,spelling}.log`。
推送前再次fetch最新main，仍是42e59b9e，已包含于当前codex功能分支。
