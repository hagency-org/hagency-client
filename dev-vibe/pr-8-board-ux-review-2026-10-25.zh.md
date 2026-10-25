# PR #8 看板体验审查（2026-10-25）

审查对象：https://github.com/hagency-org/hagency-client/pull/8 ，原始提交
`0f0fbd9cbb64c093edb7bdd4c3bb6b3405316283`，基于 `main` 的
`3e6e34f5f3f7e02d76aee4d7c019e8fc7551c2d4`。

## 判断与范围

值得合并。工具栏分组、优先级徽章、列计数、空态、加载提示与详情页层次
改善了看板可读性；改动限定在原生 Client 的五个看板文件，未修改 API、
鉴权、持久化、版本冲突或请求回执规则。窄窗口继续使用独占详情面板，
列宽 280 加 12 间距对应横向自动滚动的 292 步长。

核对了完整 diff、调用上下文、虚拟列表与拖拽占位生命周期、主题控件默认值、
已有回归和 GitHub 检查。WIP 提示沿用全部未归档任务计数；跨列满额提示警告，
同列排序不被错误标为 WIP 拒绝。

## 补充改进

- 修复页签从选中切为未选中后仍保留 `RINX_SURFACE` 悬停底色的问题。
  `script_apply_eval!` 合并属性，未选中分支必须显式恢复 `RINX_HOVER`。
- 增强已有徽章回归：拖拽源占位的徽章背景必须透明，结束拖拽后必须恢复可见底色。

## 验证记录

- 原始提交的 GitHub 桌面构建（Windows、macOS、Linux）和原生测试成功；
  移动端及独立 nightly native 项按工作流跳过，不能计为运行成功。
- 本地 i18n：2370 条目、1787 调用、0 缺失。
- 主题字面量检查通过；原生样式检查 275 文件、0 违规；工作区布局、
  定向 rustfmt 和 `git diff --check` 通过。
- 本地完整库测试：605 成功、0 失败、2 项既有忽略；其中看板套件 30 项全部成功。
- `cargo clippy --no-deps --locked -p hagency-app --all-targets -- -D warnings` 通过。
- `--nocapture` 下的部分 headless 看板夹具会输出 Makepad draw-list 代次诊断；
  测试退出成功，实际原生预览另行核对。
- 原生预览第一次链接因依赖缓存缺失 `windows.0.48.5.lib` 失败，
  改用本次任务独立依赖缓存后已恢复该导入库，但第二次构建期间刚生成的
  `libhagency` / `libserde_json` 等 rlib 又消失，预览仍未构建成功。
  因此不声称本次完成 Windows 原生视觉复核，也不将环境失败计为代码测试成功。
  原始 PR 的作者记录了明亮宽屏和深色 660×820 原生预览；本次保留其布局，
  追加改动仅恢复页签状态和增强回归断言。
- 最新提交的远端 CI 状态由 PR Checks 记录；合并须核对最终提交与检查结果。

本地编译使用 `CARGO_TARGET_DIR=D:/BuildCache/hagency-client/target`，
并通过 `cargo metadata --no-deps --format-version 1` 核对；初始依赖缓存使用
`D:/BuildCache/cargo`，预览重建改为 `D:/BuildCache/hagency-client/cargo`，
临时文件使用 `D:/BuildCache/hagency-client/tmp`。
不涉及 Docker 构建。

本次原生预览只验证布局与显示，不将设计 fixture 计为真实 Server/双用户协作验收。
