# 社区便民服务日：原生双账号协作验收

日期：2026-10-29。本机测试环境；全部场景、用户角色和数据为验收用途。

## 模拟方案

项目：社区便民服务日 · 协作演练 1029。负责人由 Local verification 担任，物资与现场执行由周予·内容与现场担任。使用两个隔离的 macOS 原生进程和真实 Matrix 账号，不连接模型。

流程：负责人建立项目并邀请执行人员；执行人员接受 Project 邀请后，单独确认 Room 邀请；筹备看板建立“确认场地”“补齐急救包”“核对物资清单”等工作项，设置负责人、标签和期限；在“统筹沟通”确认分工，在“现场反馈”报告物资问题，负责人回复处理结论；更新任务状态并完成验收。

页面：项目列表和侧栏、Overview、默认与命名 Board、默认与第二个 Room、Project/Room 成员和邀请、任务详情、双方真实消息、Agent 全局入口与 Room 成员添加入口。截图、测试日志和原始失败统一保存在忽略目录 `.run/project-simulation-20261029/`。

## 已知问题与修正

- Project 卡片重复类型文字且行高过大；去掉无意义的 Project 副标题，保留有值的主题和归档标记，并压缩卡片内边距。
- 顶部32px行容纳36px按钮造成裁切；改为按实际内容计算高度，绘制回归检查完整按钮矩形。
- 侧栏 Project 分组前增加6px间距，Board/Room子项保持紧凑。
- 既有 Room 顶栏、默认项和成员管理跟进改动一同纳入这次审查与 PR。
- [Issue #13](https://github.com/hagency-org/hagency-client/issues/13)：以上列表和工具栏布局问题。
- [Issue #14](https://github.com/hagency-org/hagency-client/issues/14)：Rooms 页签使用旧目录，错过新邀请和另一账号设置的默认项；Room 设置保存后标题、主题使用旧缓存。改为显式刷新经过鉴权的 Room 目录，并在设置事件后读取当前状态；保留账号、Room、请求和会话隔离。
- [Issue #15](https://github.com/hagency-org/hagency-client/issues/15)：发送成功后历史窗口覆盖滚动位置。保留跟随最新的意图，结束临时历史补全后恢复实时订阅；手动上翻或选择历史目标才退出跟随。回归使用实际绘制，确认返回实时后不会再次被布局自动切回历史。反向原生复测还发现 Scroll 操作直到绘制阶段才发出，逐帧重新设置 tail 会先把视口拉回底部；改为在输入处理后立即采样用户滚动，并只在成功发送/快照更新时恢复尾部，避免旧帧的 at_end 覆盖上翻意图。
- Project 列表请求期间显示加载状态，只有成功取得空列表才显示无项目；请求失败显示刷新提示。邀请弹窗的三处动态脚本调用改为 Rust 插值，消除实际日志中的脚本字段错误。

## 验收结果

实际创建了3个 Board、2个 Room 和3个任务。筹备跟进为默认 Board；现场执行与 Main Board 保持独立。统筹沟通用于分工，现场反馈用于交接。Project 邀请与 Room 邀请分别发送、分别接受，未把 Space 成员身份当成 Room 自动加入授权。

周予接受邀请、读取分工、回复物资缺口，在任务 #2 补齐急救包中设置标签、期限和检查项，推进进行中→待复核。负责人通过任务讨论核对数量、保质期和检查项，再置为完成。两个账号的 Overview 均显示3项任务、1项完成、33%完成率；剩余两项刻意保留待办，并非整个项目已完成。

最后一轮已确认跨账号改变默认 Room 后，成员点击 Rooms 获取新目录，直接进入该 Room 的邀请；管理员保存新 Room 名称和主题后，选择器和 Overview 即时显示新值。原始失败截图08、12、26保留用于对比，不作为通过证据。

最终构建完成了两条相反路径：成员主动上翻，负责人发来新消息后仍保留旧位置，显示对齐的小圆角历史提示；成员点击 Jump to latest 后最新消息可见、提示隐藏，再发两行消息完整显示，对方自动收信。截图31、41、32、33、34依次记录这些结果。Room Overview 没有全局服务管理条目，Members 保留 Add an Agent；全局 My Agents 仍列出原有两个 Agent，未连接模型或启动服务。空邀请输入显示明确错误，未向任何新账号发送邀请。

最终验证：App 全库698通过、0失败、2项原有 ignored；Server 隔离 PostgreSQL 全库134通过、0失败、0忽略；console Project Board 合同3通过。App 与 Server 自身 all-targets 严格 Clippy 通过，样式304个源文件/0违规，翻译2671条/1966调用/0缺失，30包工作区边界与双仓库 diff 检查通过。OpenAPI128个 API 操作、4个 handoff 操作、34个校验器自检通过。2项原有忽略项未被描述为通过。

原始41张 JPEG、离线索引 `index.html`、SHA-256清单 `manifest.json` 与日志保存在 `.run/project-simulation-20261029/`。截图08、12、26保留原始问题，其余为操作记录；29、30为中间修复构建，31–41为最终构建。最终 Client 源输入3a192a4be53f，原生包 `client-a21778c0-3a192a4be5-05d59a25`；Server 源输入e6f019bc6c09。第一次来源校验发现构建期间源码改变时拒绝发布，稳定源码后重建成功；没有将旧包当作最终验证。

主要命令：`cargo +1.99.0 test -p hagency-app --lib`、`cargo +1.99.0 test -p hagency --lib console::project_board`、`cargo +1.99.0 clippy -p hagency-app --all-targets --no-deps -- -D warnings`；Server 使用 `scripts/test-agent-service-postgres.py` 创建隔离数据库运行完整 suite。日志分别为 `client-full-accepted.log`、`console-contract-final-all.log`、`client-clippy-accepted.log`、`server-full.log`、`server-clippy-final.log`。

验证范围为本机 macOS 原生界面和真实本地 API/数据库，不声称完成手机设备测试或模型执行。Room 失效默认项自动修复由 Server 隔离测试覆盖，未假称通过原生界面执行不存在的 Room 归档操作。截图、账号数据库及凭证只留本机忽略目录；仓库提交代码、回归测试和本报告。

提交后 CI 复核：Server quality 首次因遗漏全仓库 rustfmt 检查而失败。已按现有格式规则整理6个相关 Server Rust 文件（含既有 Project 归档代码），`cargo +1.99.0 fmt --all -- --check` 通过；Client 同时执行 CI 的完整包选择器，发现并格式化2个 Core/console Board 文件，最终该选择器检查通过。此跟进只涉及 Rust 格式，不修改界面或协议行为；原始 CI 日志一并保留，远端后续构建与本地功能验收分别记录。
