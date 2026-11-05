# 左侧审批入口公网实测与修复

2026-11-05，macOS Apple Silicon，原生客户端连接 `https://hagency.meldry.com`。复用隔离 QA 账号和原 Agent，不创建替代身份，不修改服务器。修复前真实复现“待审批能出现，但弹窗点击穿透”；修复后待办出现、精确跳转、批准、拒绝和自然过期均通过。

## 原问题与修复

侧栏 `GlobalApprovalInbox` 已接入真实本机待审批队列。原请求出现后，左侧计数为 1，弹窗显示正确 Agent、Room 和 `model.request`；但“审阅原请求”和关闭按钮点击无响应，底层 Contacts 聊天反而响应鼠标，Esc 可以关闭。

原因是审批入口位于侧栏子树，弹窗绘制覆盖整个工作区；Home 的正常子树事件顺序却让右侧工作区先捕获鼠标。原有组件测试直接构造 ButtonAction，未覆盖真实鼠标按下与松开的事件顺序。

修复在 Home 处理输入前检查已打开的全局审批弹窗，优先把输入交给该弹窗，阻止覆盖区域下方的页面接收同一次输入。后台动作、审批轮询和生命周期事件仍遍历完整 Home 树。没有修改授权策略、原请求摘要校验、有效期或单次决策语义。

改动：

- `crates/hagency-app/src/home/home_screen.rs`：开放弹窗的输入优先级。
- `crates/hagency-app/src/hagency/ui/approval_discovery.rs`：弹窗输入接口及真实鼠标事件回归。

## 公网测试

账号 `@qa_064_20261104:hagency.meldry.com`，Agent `agt_01m6j1mvc2rfzkkt37w55t7bj0`，绑定 `bnd_01m6j1n5ejdp326dsx12tdga2b`。每条请求从聊天发送，从最左侧审批汇总打开，再核对自动选中的原请求；详情位于服务运行页下方，需要向下滚动。仅批准模型请求，不开启编程执行，也不调用文件或命令工具。

| 场景 | 结果 | 原始证据 |
|---|---|---|
| 修复前真实待审批出现 | 通过 | 计数 0→1，正确本机 Agent/Room/model.request |
| 修复前点击审阅、关闭 | **失败，已修复** | 点击穿透至底层聊天；Esc 正常 |
| 修复前原请求自然过期 | 通过 | offered1793847759→expired1793848059，真实 300 秒，无模型调用、0 token |
| 修复后空列表及鼠标关闭 | 通过 | 项目页打开“此设备没有待处理审批”，鼠标关闭成功 |
| 修复后待办出现与点击跳转 | 通过 | 从 Contacts 左侧条目进入对应 Agent 服务页；自动选中相同 Room、executionId、预览和摘要 |
| 修复后批准 | 通过 | offered→owner_allowed→applying→completed；真实回复 `SIDEBAR-ALLOW-FIXED-OK-20261105`；汇总刷新为 0 |
| 修复后拒绝 | 通过 | offered→owner_denied；聊天提示未调用模型，no-provider-call-v1、0 token |
| 修复后自然过期 | 通过 | offered1793849456→expired1793849756，完整 300 秒；详情清空、批准/拒绝按钮禁用、左侧计数为 0；点击禁用的批准无新决策；聊天和账本确认未调用模型、0 token |

批准请求 execution：`exec_806fa642a3f98d2377cdbddddca299d86e63ffb25302fe11c42cfe6b2c135966`，审批 `2073a6babbfaf9c70e92b1825490198da401812a35f0320b0103d756e95da0db`。输入 7262、输出 18，共 7280 token；cached input6016 为输入子项，不重复加算。

拒绝请求 execution：`exec_c6f2bbb451b04f6468ef8098a591070b27024c6628a10c02b7d8d268f1e4931f`，审批 `133868dedd411c066b9d2297f6b7c8a0c5cb44566d252501df20b92ef4a30573`。

过期请求 execution：`exec_41afbf7a788629a16a6dbc2e00b9e8bdb89cb0e50e7d0c9967357b6572bb6465`，审批 `9d93c47e5288feee2b1ff4d8dc2ce9cc76e8548bdb64b0f2a4b6d6c77c59fba9`，offered1793849456、到期1793849756。

## 本地检查与版本

- 新增真实 MouseDown/MouseUp 回归同时覆盖审阅与关闭，并断言下层交互控件未被按下或点击。修复后通过；仅移除 Home 输入优先处理的原事件顺序下，同一个最终测试失败（exit101）。
- 审批发现组件测试：12 passed。
- 完整 App library：886 passed、0 failed、2 项原有 ignored。
- `cargo build --locked -p hagency-app --bin hagency` 通过。
- `cargo clippy --locked --no-deps -p hagency-app --lib -- -D warnings` 通过。
- 本地构建使用原资源包，ad-hoc 签名及严格签名验证通过。链接器仍有既有 unwind 大小警告。

基础源码 main：`dfb47476019b67c1550475a0501a90e00f8eb36b`。当前 `/Applications/Hagency.app` 是本地修复构建，安装二进制 SHA-256：`605c658af648e7e9d4533d3de3dd873e85c7261b349d3a64bad5db01b19c6550`。应用版本号仍为 0.6.4；本次验证没有发布正式 Release。

新构建首次读取既有钥匙串凭据由操作者手动允许；未绕过系统 SecurityAgent。原本地构建、运行日志、只读账本快照和检查输出保留在忽略目录 `.run/sidebar-approvals-20261105/`，含认证状态的原始材料不提交。没有使用直接数据库写入、伪造待办、改时钟或缩短有效期来完成公网验收。

## 范围与收尾

本轮覆盖同一账号、同一设备、本地主人 Agent 的模型请求审批。不是服务器所有设备的审批中心验收，也不补充跨设备远程审批、多 Agent 并发、大量分页或其他操作系统的实机资格。附件工具审批与往返沿用此前独立实测结论。

收尾已确认：服务停止，runtime intents 为空，待审批 0；编程执行始终 disabled，Agent 高风险工具 Deny、模型请求 AskOwner，策略未修改。全部 16 条历史 calls settled、16 条 inbox replied，三层账户 spent 均为 1174864、held 均为 0；三层是相同消费范围，不累加。本轮新增 4 条请求：原故障现场请求自然过期、修复后批准/拒绝/过期。只有批准分支实际调用模型，新增总用量 7280 token。
