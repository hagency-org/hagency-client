# Hagency

[官网](https://hagency.org/) · [问题反馈](docs/support.md) · [数据与隐私](docs/privacy.zh-CN.md)

原生程序现位于 hagency-client 的统一工作区中，通过 `../hagency` 使用同仓库引擎。
依赖由仓库根 Cargo.lock 锁定，构建说明见[工作区指南](../../docs/workspace.md)。
Windows 使用仓库根 `tools/dev.ps1`，将构建产物及缓存放到 D:/BuildCache/hagency-client。

Client 0.6.0 已发布，下载见 [Hagency Client Releases](https://github.com/hagency-org/hagency-client/releases/latest)。[发布检查报告](../../dev-vibe/desktop/release-0.5.0/assessment.zh.md) 记录的上游依赖安全项仍待处理，当前限制见 [0.6.0 发布说明](https://github.com/hagency-org/hagency-client/releases/tag/v0.6.0)。

[English](README.md) | 简体中文

Hagency 是原生 Rust/Makepad 客户端，将 Matrix 聊天、Project 工作区和
本机 AI Agent 管理整合到同一个应用。当前登录只支持 **Hagency 服务器**：填写
服务器地址，通过它的 Pasion 服务在浏览器登录。普通 Matrix 服务器不能作为
当前登录入口。

## 管理边界

- 一个 Project 绑定一个 Matrix Space；讨论 Room 各自维护成员。
- Agent 是服务器级身份，由创建者永久拥有。新建时默认绑定当前已授权设备，
  接入 Room 是独立操作，不要求先选 Project。
- 服务器校验身份、Room 接入权和执行设备。Project/Room 管理员设置 Agent
  接入权限与服务暂停。
- 主人在自己的设备上选择 Codex、模型、工作目录、token 配额和工具策略，
  无需服务器管理员审批资源消耗。
- 一台设备可以执行多个 Agent。切换账号会停止当前主人的本地运行，不转让
  Agent，也不合并账号数据。

## 快速开始

1. 输入 Hagency 服务器地址，在浏览器完成 Pasion 登录。这一次授权同时建立
   Matrix 聊天会话和原生主人管理会话。
2. 打开 **Projects → New Project**，或使用 Projects 右侧 **⋯**。默认创建新
   Space，也可以选择已经加入的 Space；从详情添加讨论 Room。
3. 打开 **Agents → New Agent**，填写名称，等待身份及主人私聊就绪。创建不带
   Project/Room，执行设备默认为当前已授权设备。
4. 在 **Codex** 选择 **Use my signed-in local Codex account**。
   本机已登录的 Codex 可以直接复用，无需再次登录；也可使用独立 Codex 登录。
   选择模型和工作目录并保存。
5. 在 **Budgets** 设置 Agent、Room、Room 内请求者三层策略。
   在 **Run** 选择主人私聊或已绑定 Room，明确选择估算记账、
   单次预留量，再启动该 Room。

详见[原生快速开始指南](docs/hagency-quickstart.zh-CN.md)。界面语言可切换；
上面的英文名称对应当前源码控件，部分管理页文字尚未完成中文翻译。

Project 详情 tab 为 **Overview / Rooms / Members / Access**；Agent 详情为
**Agent / Codex / Budgets / Run**，分别管理身份与设备、本机资源、额度与请求
策略，以及选中 Room 的运行与审批。

## 导航与保留功能

桌面侧栏提供 **Projects、Agents、Chats、Contacts、Mini apps**；前三项右侧
**⋯** 提供创建入口。**More** 中保留 Discover、Explore rooms、Moments、File
Transfer、Article editor、Browser、Settings。Project 条目的 **⋯** 可打开概览、
讨论 Room、成员、邀请成员以及绑定 Space；邀请按钮遵循真实 Matrix 权限。

聊天、联系人、搜索、附件、普通用户加密聊天、朋友圈、原生文章编辑器以及
小程序导入／权限审阅／运行均保留。默认小程序目录不再提供 Palpo 应用。
Hagency 管理由原生 Projects/Agents 页面承担，旧资源与协调者审批流程已移除。
当前运行链支持主人私聊中的加密任务、Codex 编程执行、加密回复和文件收发，
前提是通过已验证的主人设备核验并验证该 Agent 的精确身份。新建主人私聊默认
加密；已有明文私聊需要显式升级。预算、工具策略和显式启动服务仍然生效。

共享 Project 中其他请求者的加密执行、跨主机密钥恢复尚未交付。现有端到端
验收覆盖 macOS，并使用隔离 OAuth 测试桥接；人工桌面操作、生产 OAuth 和
其他平台的完整资格验证仍未完成。这不代表 Pasion 浏览器登录不可用。
实现路径及验证边界见[主人加密链路验收记录](../../dev-vibe/agent-encrypted-codex-validation-2026-10-27.zh.md)。

## 账号、设置与本地数据

通过 **Settings → Account** 的账号列表添加或切换账号；左下角账号菜单可打开
账号设置和退出登录。每个账号保留独立 Matrix 设备、加密数据库及主人配置，
只有当前账号同步。切换停止原账号 Agent 运行；退出会停止运行并撤销当前授权。

设置分为 **Account、Preferences、Privacy、About**；外观与语言在 Preferences。
通用外部 **Agent Access** 工具与原生 Hagency Agents 管理不同，配置 Hagency
不需要填写 BotFather 或 loopback 服务地址。

`HAGENCY_DESKTOP_DATA_DIR` 可指定绝对路径以隔离数据。启动器仍识别
`RINX_DATA_DIR`、`ROBRIX_DATA_DIR` 环境别名，但这不代表兼容已退休 Fleet 数据。
账本、工作目录及 Codex 关联按完整授权身份隔离。换执行设备不会转让所有权、
清零成本或重跑未知执行；恢复检查要求时，必须恢复同一主人的完整账本。

## 构建与开发

Desktop 与主人引擎同属 [`hagency-client`](https://github.com/hagency-org/hagency-client) 工作区，使用 `../hagency` 本地路径依赖和仓库根 Cargo.lock。以下命令在 `crates/hagency-app` 执行；Windows 优先使用仓库根的 `tools/dev.ps1`。安装 CMake 和平台依赖后执行：

```sh
cargo run --locked --bin hagency
cargo test --locked --lib
```

仓库固定 Rust 1.98.0，Cargo 包名为 `hagency-app`、可执行文件为 `hagency`，应用显示为 Hagency。
引擎作为桌面共用库保留，原独立 CLI 和 Web 控制台已移除。
默认 standalone 构建已包含原生管理，不需要开启旧 `agent_chat` 仪器功能。
移动打包需要签名及实机验证。

内置应用目录是 `system-apps.json`，文章编辑器位于 `apps/article-editor/`。
见[应用开发指南](apps/README.zh-CN.md)。完整构建、致谢与许可证信息见
[英文文档](README.md)。

另见 [开发、隔离测试目录与原生验收](docs/local-development.zh-CN.md)。

[GitHub CI 与发布](docs/github-ci.md)
