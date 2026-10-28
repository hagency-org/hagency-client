[English](README.md) | [中文](README.zh-CN.md)

# Hagency 客户端

[Hagency](https://github.com/hagency-org/hagency-client) 是 Windows、macOS 和 Linux
原生桌面应用，集成 Matrix 聊天、Project 工作区和本机执行的 AI Agent。
安装包见 [Client Releases](https://github.com/hagency-org/hagency-client/releases/latest)。

通过 Pasion 登录 Hagency 服务，在应用内管理项目和 Agent。使用步骤见
[桌面快速开始](crates/hagency-app/docs/hagency-quickstart.zh-CN.md)和
[应用说明](crates/hagency-app/README.zh-CN.md)。

应用和本地 Rust 引擎位于同一个 Cargo 工作区。默认包为 `hagency-app`，
可执行文件为 `hagency`；`crates/hagency` 作为引擎库由桌面进程直接调用。
旧 CLI Web 控制台、网页原型及无界面服务安装入口已移除。桌面登录所需的本机
OAuth 回调保留，但该端口不提供管理页面或管理 API。

## 开发

macOS 完整本地联调：在相邻的 Client / Server 仓库准备好依赖后执行
`./tools/local-dev.sh start --pull`，更新两个 main 并启动 PostgreSQL、完整后端和原生客户端。
所有前端和二进制按源码指纹构建、校验，使用 `./tools/local-dev.sh doctor` 复核运行资源。
服务器地址为 `http://127.0.0.1:8088`；账号、日志、停止与重启见
[一键本地开发指南](docs/local-development.zh-CN.md)。

Windows 的构建产物、依赖缓存和临时文件必须留在 D 盘。包装脚本会设置并核对：

```powershell
./tools/dev.ps1
./tools/dev.ps1 check --locked -p hagency-app --lib
./tools/dev.ps1 test --locked -p hagency-app --lib
```

macOS/Linux 按[原生依赖说明](crates/hagency-app/docs/local-development.md)准备环境，
运行 `cargo run --locked -p hagency-app --bin hagency`。不再需要 Node 页面构建或
另起控制台进程。工作区、测试和安装包流程见[工作区指南](docs/workspace.md)。

原 `hagency-org/hagency-desktop` 仓库保留历史源码和发布；当前开发集中在这里。
历史 Fleet、CLI 文档仅作旧实现记录，不作为当前安装说明。

## 许可

Apache 2.0。保留 [LICENSE](LICENSE)、[NOTICE](NOTICE)及[第三方许可](docs/LICENSING.md)。
