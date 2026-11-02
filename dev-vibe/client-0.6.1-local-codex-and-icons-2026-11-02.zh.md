# Client 0.6.1：本地 Codex 检查与图标缺失

## 用户可见结果

- 现场 CLI `codex login status` 返回 `Logged in using ChatGPT`，Hagency 使用相同 Codex 可执行文件。刷新页面后、补资源前，客户端已显示使用本机登录账户；补资源重启后仍显示同样状态。没有重新登录 Codex，也没有执行模型任务。
- 仅凭原截图不能判定当时是实际 Hagency 设备授权失效还是临时服务端检查失败。已确认旧代码会把 owner API 的所有错误（含 503、502、403）改成 401 `owner_authorization_required`，因此“账号过期”的文案不能证明 Codex 退出了登录。
- 安装包缺失 `hagency_app/resources` 是图标消失的直接原因。补入原始 v0.6.1 的 139 个桌面资源后，侧栏、刷新、附件、设置和发送图标恢复，账户和聊天保持可用。

## 源码修复

`owner_provider.rs` 的四个 owner API 检查保留 HTTP 状态，并只输出固定诊断代码：401 授权检查失败、403 权限不足、502 响应异常、429/其他 5xx 暂时不可核验。保留每一步原有的设备有效期、身份一致性和撤销检查，不自动重试写操作。界面区分 Hagency 与本地模型账户，增加对应中英文说明。

资源收集器 `robius-packaging-commands 0.4.1` 按 `--binary-name hagency` 输出 `dist/resources/hagency/resources`；Makepad 的 Rust crate 资源命名空间和 packager 清单均要求 `hagency_app`。打包前逐文件校验原始资源存在且字节一致，再规范化到 crate 命名空间；资源缺失或被改写时中止打包。Android/iOS 专用目录沿用上游桌面排除规则。

## 本机修复与发布边界

- 从发布 tag v0.6.1 提取资源，经过同一验证/规范化函数后，补入 `/Applications/Hagency.app/Contents/Resources/hagency_app`，正常退出并重新启动。
- 主程序 SHA256 保持 `086d8af567106db1c92339342c4bfd53dcdc6cc3d9592d95dff19f20d13b58a6`；本机应用现在属于补齐资源后的安装副本。
- 原 GitHub v0.6.1 安装包未替换，本机仍运行原始 0.6.1 主程序；授权错误分类修复需要后续重新构建/发布才会进入安装程序。
- 未运行完整跨平台安装包构建，未执行付费模型任务。

## 验证

- 先复现后修复：旧资源脚本 5 项中 2 fail / 2 error，修复后 5/5；原 owner API 集成测试新增 503 用例后在旧代码失败（实际错误 401），修复后全部 503/502/403/401 分支及原有到期/撤销流程通过。
- package-octos 6/6；UI 新增诊断测试 1/1；中英文目录 2815 项、2085 调用点、0 缺失；335 源文件样式、主题和 31 包边界检查通过。
- 精确 CI feature App library：885 passed / 0 failed / 2 pre-existing ignored；hagency-app、hagency、rinx-miniapp-core all-target strict Clippy 通过。App 测试链接器提示大型 unwind section 警告，编译及测试均成功。
- 证据在本 worktree 忽略目录 `.run/fix061/`，包含 baseline/final 日志、本机资源校验清单和两张恢复截图。没有在报告中保存账户凭证。

没有 provisioned task-writer 或 project.spec.md；此轮使用隔离分支 codex/fix-061-auth-and-resources，保留其他工作树的现有修改。
