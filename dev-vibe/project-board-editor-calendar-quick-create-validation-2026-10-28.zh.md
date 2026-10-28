# 看板 Markdown 选区、日期与快捷创建（2026-10-28）

范围：Client `main` 的原生看板。复用现有 Server 看板 API，不改身份、权限、数据库或业务协议。

## 根因与修复

原生窗口实际复现了鼠标拖选失败，日志同时出现 `can't move cursor because layout was invalidated by earlier event`。看板在每个 Actions 批次结尾同步控件；`sync_markdown_controls` 每次无条件调用 `TextInput.set_is_read_only`。固定版本 Makepad 的该 setter 无论值是否改变都会清除 `laidout_text`。悬停、焦点和拖动事件之间未必有新一轮布局，后续指针定位因此直接失败。

现在只在只读状态真正变化时调用 setter，继续保留保存、归档和冲突期间的编辑锁；没有修改依赖缓存。新增回归在绘制后连续同步控件、不重绘，检查中文选区、依赖布局的光标移动、删除和写入锁。光标使用主题强调色，选区明确绑定主题选中文字色。编辑页签改为“编辑 Markdown”，移除看起来像可点击开关的静态 Markdown 标签，保留预览和原生撤销/IME 管线。

## 行为

- 截止日期右侧日历图标打开月历，可翻月、选日、点今天、清除或关闭；没有截止日期时定位并高亮本机当天，已有日期时定位其月份。打开或关闭月历本身不写入截止日期，只有明确选日才赋值。日期为日历日期，无时区时间戳转换。
- 每列底部“+ 添加卡片”和列头“+”展开列内标题输入框。按 Enter 或“添加卡片”创建，取消/Escape 收起。顶部“新任务”仍打开完整详情。
- 快捷卡片仅传标题、目标列及协议必需的普通优先级；描述、负责人、标签、日期、父任务、清单均为空。后续点击卡片进入详情补充。
- 同一时间只编辑一张快捷卡片。有详情草稿或列设置时不能开启；快捷草稿打开时不能切换详情或配置列。保存沿用原有幂等键、未知结果保留与重试规则，成功前不清空标题，后台拒绝/WIP 超限保留输入。成功后不替换当前打开的另一任务详情。

## 自动检查

- `cargo +1.99.0 test --locked -p hagency-app --lib hagency::board --no-fail-fast`：35 通过、0 失败；覆盖选区/布局、闰年/跨年/六周月份、日期可选性、标题与元数据边界、未知结果重试、详情隔离，以及既有看板回归。
- `cargo +1.99.0 clippy --locked -p hagency-app --lib --tests --no-deps -- -D warnings`：通过。
- 翻译检查：2556 条、1931 调用、0 缺失；原生样式审计 294 文件、0 违规；工作区边界和 `git diff --check` 通过。
- 系统 `python3` 是旧版，不含 `tomllib`；工作区边界检查使用已安装的 `python3.12` 通过。没有改动系统 Python 或项目依赖。

以下验收使用完整原生客户端与真实本地 Server；headless 测试仅作为回归检查。

## 原生实际操作与持久化

- 使用现有真实本地环境、OAuth 账号和 `Local startup verification` Project；没有 fixture 身份、认证替身或直接写数据库。
- 列底部展开后直接键入标题，自动获得焦点；按 Enter 创建真实 #2 卡片 `Inline card · editor verification`。创建后只读 SQL 确认 `version=1`、`state=todo`、`priority=normal`，description 为空、assignee/due_date/parent 为空，labels/checklist 为空数组。
- 打开 #2 详情：日历默认高亮 2026-10-28；前后翻月确认 2026-11/2026-10，点选当天得到 `2026-10-28`。清除并保存后 SQL 确认 `version=3`、due_date 为 NULL；最终重新选当天保存，留作演示。
- Markdown 实际输入中英文三行，鼠标拖选 `DELETE-ME` 并 Backspace，只有所选子串消失，Cmd+Z 恢复；再拖选中文“选区删除”、删除、撤销，边界正确。点击加粗保留中文选区，预览正确渲染标题和粗体。实际操作之后日志新增区间没有布局失效警告。
- 保存后 SQL 确认原文为 `# Markdown verification\nKeep DELETE-ME keep\n中文**选区删除**与撤销`，当天日期落库；测试删除后已撤销，示例原文因此保留 DELETE-ME。
- 最终清理日历图标按钮的继承默认文字，并以相同源码重新执行 35 项看板测试与严格 Clippy，通过后重建启动。重启账号恢复，原有 #1 和本次 #2 均保留；重新打开 #2 确认描述与清除日期结果，再选当天保存至 version=4，并再次实际拖选。
- 最终来源绑定：Server `d5093e80e14a` / 输入 `bccee573472b`，Client `20fc93234926` 加本次未提交修改 / 输入 `befa224fd365`。Client 运行包 `client-20fc9323-befa224fd3-fb048c12`；不是运行旧构建后仅修改源码。

截图位于 Server 忽略目录 `.run/local-dev/verification/`：`board-inline-created-20261028.png`、`board-markdown-selection-20261028.png`、`board-calendar-final-20261028.png`、`board-editor-final-20261028.png`。测试数据、日志、原生构建和认证配置均留在既有隔离运行目录，不写入代码或文档。仅验证本机 macOS 原生操作，其他平台未在本轮手动运行。

## 提交范围

本次向 Client main 提交原生看板代码与回归测试、日历 SVG 源文件、英中文案，以及本验收记录和工作区知识/进度文档，共 15 个文件。按明确路径清单暂存；提交审计排除 `.run/`、数据库、账号/密码/令牌配置、日志、截图和 `target/` 构建产物，并核对本地运行凭据未出现在待提交内容中。上面的运行包与来源信息记录提交前的实际验收批次；源码提交本身不改写运行数据。
