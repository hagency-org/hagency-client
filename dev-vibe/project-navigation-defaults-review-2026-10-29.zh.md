# Project 导航、default 与 Room 层级复核（2026-10-29）

Matrix 的 Space 是 type=m.space 的 Room。标准 Space 层级中，父节点必须是 Space；本产品只允许 Project Space → 直属普通 Room。创建沿用该 Space，关联只登记已属于该 Space 的普通 Room，并校验父 Space、非 Space 子项及直属关系。下拉改为 Connect an existing Room，说明已澄清。

原生确认 Boards/Rooms 进入实际默认项、自动展开对应 Project、选中状态随 Overview/Board/Room 切换；两个默认项分别设置成功、排在其类型首位，badge 与下拉均为小写 default。Room 默认设置经重启保持。普通成员没有设置默认项的操作。真正没有 Room 时，管理员点击 Rooms 通过原创建日志建立 General 并直接打开实际聊天；无权访问的私人 Room 不会触发替代创建或自动加入。

首次原生测试发现新建 Room 已成功但 SDK 缓存尚未更新时，通用 RoomId 路由会弹出多余加入确认。保留原始截图03，取消该确认后修正为采用经过鉴权的目录成员状态进行 JoinedRoom/InvitedRoom 导航；最终新建另一项目复测通过，见截图16。

默认项失效修复由 Server 持久化：inactive、移出 Space、tombstone、零→首个 Room、最后 Room→无 Room；避免 Board 读取复活已移除 Room，且不会因某个用户看不见私人 Room 改动全局默认。隔离 PostgreSQL 回归覆盖上述状态、重开数据库、权限、版本冲突、跨项目拒绝和并发更新；本轮未冒称原生点击了不存在的 Room 归档操作。

最终 Client 全库696通过、0失败、2个原有ignored；Server隔离全库134通过、0失败、0忽略。自身all-targets严格Clippy、OpenAPI128+4操作及34自检、样式304/0、翻译2671条/1988调用/0缺失、30包边界、双仓库diff及最终来源doctor通过。无模型连接、Agent启动或审批决定。

原始16张JPEG、可离线查看的index.html、SHA-256清单manifest.json及构建/验证日志位于忽略目录 `.run/navigation-defaults-20261029/`。最终运行 Client client-a21778c0-7b922137e2-f0bdafaa，Server server-9d17c255-e6f019bc6c-ab50b7d3。当前跟进改动保留未提交，前次已推送批次记录不变。

协议参考：https://spec.matrix.org/latest/client-server-api/#spaces 、https://spec.matrix.org/latest/client-server-api/#mspacechild 。
