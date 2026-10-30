# 桌面端 owner 引擎

本 crate 是 `hagency-app` 的本地库依赖。桌面创建
`native_owner::NativeOwner::open_with_matrix`，在同一进程内执行类型化命令。
登录、账号隔离、Agent 执行、额度、审批、恢复、模型和语音逻辑继续保留。

实际 HTTP 监听仅用于随机本机端口上的 OAuth 回调
`/console/server-login/callback`，不提供管理网页或管理 API。
内部请求路由继续执行既有的身份、权限和策略检查，不通过网络端口暴露。

不再提供产品 CLI、静态 Web 资源包、无界面用户服务安装器或 CLI 发布流程。
`tests/fixtures` 中显式声明的 probe 二进制仅用于测试。

使用说明见[桌面指南](../hagency-app/README.zh-CN.md)和[工作区指南](../../docs/workspace.md)。

原生 Agent 与独立 server-login 持久化路径使用和桌面相同的显式系统凭据后端。
Linux 因此需要可用的 Secret Service 与用户 D-Bus 会话；无此服务的无界面或
容器环境会明确失败，不会静默改用文件或 Mock。上述是库路径，不表示恢复了
无界面产品入口。本批尚未验证真实 Linux 服务/容器凭据行为，自动测试使用显式
隔离假存储。详见[凭据存储与恢复](../hagency-app/docs/privacy.zh-CN.md)。
