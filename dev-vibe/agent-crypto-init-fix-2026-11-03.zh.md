# Agent 加密身份初始化：诊断与跨组件修复

2026-11-03。本次由用户授权定位、修复、提交 PR、复核合并，并验证一次最小 Codex 对话。没有新的版本发布授权，不覆盖 v0.6.2 资产。

## 现场与根因边界

公开原版 0.6.2 已证明 Codex 本机账号识别及图标恢复。既有公网 QA Agent 的原 SDK 目录为空，身份初始化失败，尚未调用模型。以相同 QA 会话、只添加白名单错误码展示的本地诊断构建复现：原凭据经用户手动允许钥匙串访问后，身份状态为 `agent_crypto_request_refused`；Service 保持 stopped。此前等待系统钥匙串期间出现的通用 `agent_crypto_prepare_failed` 也保留，不能用之后的结果改写原始观察。

服务端源 1c95949 的内置 Appservice 注册未开启 `io.element.msc4190`；新增必需设备能力断言在原代码上实际失败。当前 Palpo main 814b84c3 的设备 PUT 则使用随机设备 ID，而客户端/Server broker 固定绑定请求中的 ID；实际 HTTP + 独立空 PostgreSQL 回归在原代码上失败，观测随机 ID 与 `HAGENCY_EXACT_DEVICE` 不同。

仅修正随机 ID 的中间实现又暴露原调用把 Appservice token 写入普通用户 token 表的问题：后续越界 user_id 的请求错误返回 200，而测试要求 403。原失败已保留。上游修正改为仅创建 owner/device 元数据、保留指定 ID 和显示名、并发插入幂等，不创建普通用户 token；固定 ID/whoami/重试/双虚拟用户/越权拒绝/缺设备能力拒绝实际回归已通过。没有通过调换身份或弱化 namespace 检查隐藏该失败。

## Client 改动

- 身份失败时展示固定白名单诊断码；未知字符串、URL、凭据样文本均不显示。
- 对服务器拒绝给出独立 Hagency 加密请求说明，避免混成本地 Codex 登录问题。
- Native 生命周期保留两个固定的协议格式错误码；其他错误仍受原安全筛选，不显示原始响应。
- 不改变 SDK 身份、私有存储、权限、任务、模型或发送逻辑。服务端及 Palpo 的修复必须另行落地，单独更新 Client 文案不能修复公网身份初始化。

## 已执行验证

- Client 默认 feature App library：879 passed / 0 failed / 2 既有 ignored。
- Native Agent crypto：9 passed / 0 failed / 0 ignored，包含原密钥保留、丢失拒绝及取消/停止收尾。
- i18n 2816 项 / 2086 调用点 / 0 缺失；335 源文件样式 0 违规，31 包边界检查通过。
- 精确 App CI feature 的完整 library 及严格 Clippy 继续验证，以最终日志为准。
- Server 本地 library：25 passed / 0 failed / 1 新的显式数据库用例 ignored；该数据库用例已对独立空数据库另行实际运行并通过，覆盖并发幂等、完整注册身份及禁用状态保留。
- Palpo 默认 library：310 passed / 0 failed / 72 ignored；所改 HTTP 数据库用例另行运行，不把普通套件的 ignored 算通过。最终 default/all-features 用例及 hosted CI 继续验证。

## 当前限制

三组件 PR 尚待最终复核/合并和服务端部署，实际加密身份验证及固定文本 Codex 回复尚未验收通过。本次模型任务发送数仍为 0；不声称端到端已修复。保留用户既有凭据、身份和所有原失败。

证据分别保留在三个隔离 worktree 的 `.run/agent-crypto-init/` 与 `.run/agent-device-init/`；包含原始失败和阶段日志。桌面截图位于本聊天工具输出，未宣称存在本地 PNG。没有 provisioned task-writer 或 project.spec.md；其他 worktree 保持原状。
