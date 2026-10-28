# 桌面客户端与完整后端本地开发

## macOS 一键启动

Client 与 Server 必须是相邻的 `hagency-client/`、`hagency-server/` 源码仓库。
安装并启动 Docker Desktop，准备 Python 3、Rust 1.99.0、Xcode 命令行工具、
CMake 和 libpq。首次安装 Rust 工具链可运行 `rustup toolchain install 1.99.0`。
在 Client 仓库根目录执行：

```sh
./tools/local-dev.sh start --pull
```

`--pull` 先对两个仓库执行 `fetch origin main` 和快进合并；必须位于 `main`，
遇到分叉或会覆盖本地修改时明确失败，不自动 reset、stash 或强推。
已更新源码后也可不带参数，默认 `start` 使用当前 checkout。
脚本自动生成独立配置与随机密码、启动 PostgreSQL，并从同一份 Server 源码和
Cargo.lock 完整构建后端、管理前端和 Pasion 身份前端，再构建原生 Client。
等待 Appservice 实际启动收发探针成功、确认看板能力，且验证实际返回的页面、
共享主题、图标、脚本和 WASM 字节属于本次构建后，才报告启动成功。
首次下载依赖和编译可能需要较长时间；之后复用构建缓存和原有测试数据。
启动错误会指出日志文件，不使用登录或 Matrix 测试替身替代失败的服务。

## 地址和登录

| 用途 | 地址 / 值 |
| --- | --- |
| 桌面登录页的服务器地址 | `http://127.0.0.1:8088` |
| 浏览器管理页面 | `http://127.0.0.1:8088/login` |
| Pasion issuer | `http://127.0.0.1:8088/auth/` |
| Matrix 身份域 | `localhost:8088` |
| PostgreSQL | `127.0.0.1:55438` |
| 首次创建的本地管理员 | `admin`，完整 ID 为 `@admin:localhost:8088` |

桌面端输入服务器地址时不要追加 `/login`、`/auth/` 或 API 路径。
点击登录后在浏览器完成真实 Pasion 授权；桌面用随机 loopback 端口接收 OAuth 回调。
本地管理员密码只保存在 Server 仓库的 `.run/local-dev/admin-password`，
可在终端用 `cat ../hagency-server/.run/local-dev/admin-password` 查看。
脚本不会每次重建管理员或重置密码。一键测试环境默认开放注册，可在
`http://127.0.0.1:8088/auth/register` 创建普通测试账号；邮箱、手机号可留空。
此默认值仅用于隔离的本机环境，Server 部署模板仍默认关闭注册。
已有环境的配置会保留；修改 `.run/local-dev/pasion.toml` 的
`[account].password_registration_enabled` 后用 `restart` 生效。

完整 Server 内包含 Pasion 登录、Palpo Matrix、Project、Agent 和看板 API；
桌面登录后从“项目 → 项目详情 → 看板”进入。`project-board-v4` 是当前看板能力标识。
模型调用仍需在客户端配置自己的模型账号并显式启动 Agent。

看板工具栏的 New column 可新建列；输入名称后点击 Save columns。拖动列头的
`≡` 手柄调整顺序。在 Columns & WIP 选择列，点击 Archive / Restore 后
用 Save columns 提交。归档列会隐藏，其卡片、负责人、评论和排序保留；恢复列后
重新显示。已归档列内的卡片不可编辑或移入，须先恢复列。最多保留 12 列，
且至少有 2 个活动列和 1 个活动完成列。

任务详情中更改 Status 后点击 Save，会将状态与其他字段一次性保存；WIP 上限、
版本冲突或权限拒绝时不提交部分字段，弹窗保留草稿。列排序和归档使用看板版本
防止双端互相覆盖。Client 和 Server 需要一起升级到 v4。

HTTP 开发地址只绑定本机 loopback，不需要域名、hosts 或证书设置。
历史的 `https://hagency.local` 属于另一套 HTTPS 部署；本脚本不启动该域名，
也不复用或迁移它的身份、签名密钥和数据卷。命名域名联调需要独立的可信 HTTPS
配置，见 [Server 本地部署说明](../../hagency-server/docs/LOCAL_DEPLOYMENT.zh-CN.md)。

## 停止、重启与排障

```sh
./tools/local-dev.sh status                 # 进程、就绪及日志位置
./tools/local-dev.sh doctor                 # 校验源码、产物和实际 HTTP 资源
./tools/local-dev.sh update                 # 只更新两个 main，保留本地修改
./tools/local-dev.sh logs                   # 最近的服务、客户端和构建日志
./tools/local-dev.sh stop                   # 停止本脚本启动的前后端和数据库，保留数据
./tools/local-dev.sh restart                # 重建变更并重新启动
./tools/local-dev.sh restart --rebuild-assets # 强制重建完整产物，包括两个 WASM 应用
./tools/local-dev.sh start --server-only    # 仅完整后端，不打开原生客户端
```

`start` 自动识别源码/分支变更并构建、切换到新版本；没有变化时校验后复用进程。
`restart` 即使源码未改变也重新启动进程。构建在替换运行中的服务之前完成，
构建失败不会把不完整产物发布给服务。
构建和运行日志在 Server 的 `.run/local-dev/{build,server,client}.log`。
配置、密码和后端签名密钥也在 `.run/local-dev/`；不要提交或删除它来处理启动失败。
PostgreSQL 使用独立 Compose 项目 `hagency-full-dev` 和数据卷
`hagency-full-dev_database`，三库分别是 `hagency`、`palpo`、`pasion`。
原生客户端测试账号、缓存和资源使用
`~/Library/Application Support/org.hagency.local-development/`，与个人账号隔离。

## 防止资源混用

每批构建保存在 Server `.run/local-dev/builds/` 的独立目录，包含
`build-manifest.json`：仓库绝对路径、分支、HEAD、工具链、实际源码内容指纹，
以及所有二进制和静态资源的 SHA-256。生成目录与数据库、账号目录分开。
已存在的 `resources/`、旧镜像中的 assets、单独复制的 WASM 均不能成为一键环境
的构建来源。缓存必须通过来源、完整性和文件校验才能使用。

Server 管理页和 `/auth/*` 继续按主干设计共用 `/assets/hagency-theme.css`、
`hagency-theme.js` 和 logo；认证流程仍复用固定版本 Pasion 前端。
关闭注册且无第三方注册方式时，注册页显示明确说明、账号获取建议和返回登录按钮。
Pasion 页面修复保存在 Server `xtask/patches/`，只应用到独立构建副本；
不修改 Cargo 缓存、上游 checkout 或业务认证协议。

`doctor` 同时核对磁盘产物与 HTTP 实际响应，API 健康不能代替页面资源验证。
若校验失败，先查看日志并运行 `start`；它会从当前源码完整重建。
构建期间源码变化会导致该批产物被拒绝，需要重新运行。文档和进度记录不计入编译输入。

如果 8088 或 55438 被其他环境占用，先确认归属并停止冲突环境，再运行启动命令。
脚本不会接管未知进程、覆盖已有配置或删除数据卷。若私有配置丢失而数据卷还在，
必须恢复匹配配置；不要重新生成密码连接旧数据。

历史看板 `project_board_fixture` 使用 `http://127.0.0.1:55474`：
它的项目/看板 API 和 PostgreSQL 是真实实现，Pasion/Matrix 身份是测试替身。
它只用于隔离协议测试；上面的一键环境运行完整 Server，已包含看板。

## 其他开发方式

桌面端位于 `crates/hagency-app`，构建与缓存要求见[工作区指南](workspace.md)。
Windows 使用 `./tools/dev.ps1`，macOS/Linux 原生依赖见
[原生开发指南](../crates/hagency-app/docs/local-development.md)。
Server 手动部署见 [Server README](../../hagency-server/README.zh-CN.md)。
测试登录协议可运行 `./tools/dev.ps1 test --locked -p hagency-app --lib login::oauth_tests`。
