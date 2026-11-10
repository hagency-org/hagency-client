[English](README.md) | [中文](README.zh-CN.md)

# Hagency Client — 已归档

Hagency Client 已并入 [Hagency Desktop](https://github.com/hagency-org/hagency-desktop)。此仓库仅保留历史源码、发行版本和讨论，不再承接开发工作。

- [下载 Hagency Desktop](https://github.com/hagency-org/hagency-desktop/releases)
- [提交问题和功能需求](https://github.com/hagency-org/hagency-desktop/issues)
- [参与原生客户端库开发](https://github.com/hagency-org/hagency-desktop/tree/main/crates/hagency-client)
- [Desktop 编译与开发说明](https://github.com/hagency-org/hagency-desktop#readme)

18 个原生库、后台服务接入、测试和发布流程均由 Desktop 仓库维护。迁入工作已通过 [Desktop PR #11](https://github.com/hagency-org/hagency-desktop/pull/11) 合并，旧独立 UI 已移除。后续修复请提交到 Desktop。

未结问题已连同讨论记录迁移：

| 原 Client 问题 | Desktop 问题 |
| --- | --- |
| #4 — macOS 更新 | [#44](https://github.com/hagency-org/hagency-desktop/issues/44) |
| #23 — 按钮文字与图标对齐 | [#45](https://github.com/hagency-org/hagency-desktop/issues/45) |
| #24 — 房间加密选项 | [#46](https://github.com/hagency-org/hagency-desktop/issues/46) |
| #25 — Matrix 设备认证与恢复 | [#47](https://github.com/hagency-org/hagency-desktop/issues/47) |
| #29 — 移动端编译缺口 | [#48](https://github.com/hagency-org/hagency-desktop/issues/48) |

Client [PR #52](https://github.com/hagency-org/hagency-client/pull/52) 的私聊房间创建恢复修复已通过 [Desktop PR #49](https://github.com/hagency-org/hagency-desktop/pull/49) 移交 Desktop。

旧工作区中未提交的测试修改已保存到 [`archive/startbug-test-corrections-20261110`](https://github.com/hagency-org/hagency-client/tree/archive/startbug-test-corrections-20261110) 分支。它是历史补丁，不代表已通过验证的 Desktop 发布改动。

历史发行版本继续保留供查阅。当前软件下载和支持请使用 Desktop。本仓库保留原有许可证和声明。
