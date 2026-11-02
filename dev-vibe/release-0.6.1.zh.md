# Hagency Client 0.6.1 发布记录

正式公开发布已完成（2026-11-02 18:26:47 +08）：[Hagency Client v0.6.1](https://github.com/hagency-org/hagency-client/releases/tag/v0.6.1)。Release id431322200，draft=false、prerelease=false，releases/latest为v0.6.1。标签精确指向实际构建提交51f8eadf6956571a1e25426f94715b4afda4bab1。

- 来源：[成功构建41390501008](https://github.com/hagency-org/hagency-client/actions/runs/41390501008)。三平台引擎和五平台安装包任务全部SUCCESS；真实环境qualification及自动publish两个条件skip不计通过。复用原始产物，未重新编译或改写安装包。
- 七安装包共1006460017字节，逐个重算SHA256与原五平台清单一致；生成总清单后，上传草稿的13个附件名称、大小、uploaded状态及GitHub服务端SHA256摘要逐一匹配。总清单通过认证草稿下载和匿名公开下载两次回读均字节一致。七个Latest安装包匿名入口全部HTTP200。
- 两个macOS DMG独立CRC验证通过；Linux两份DEB控制元数据确认0.6.1和对应amd64/arm64；两份AppImage ELF机器类型正确。只读挂载DMG被本机OS拒绝，不声称已安装/启动；Windows启动壳PE机器类型不代表内嵌应用架构，保留实际amd64 CI构建来源，未将最初过强的启动壳断言误报为包缺陷。
- 发布说明保留未签名、手动安装、没有签名自动更新及两项真实环境记录仍是placeholder的限制。tag推送自动触发的重复正式流水线41428469625已取消，其结果不是通过；本次产物资格仍来自成功41390501008。

以下保留准备阶段范围：

- 用户明确选择沿用0.6.0的未签名、手动安装发布方式，公开说明缺少正式签名和两项真实环境验收；不把占位验收或跳过的发布检查记为通过。
- 基于main091931f21，版本同步至workspace、App、21个本地包锁项、macOS bundle和占位记录的版本pin；第三方依赖零变更。
- 发布前执行现有app-release.yml的publish=false构建，保留三平台engine和五平台安装包自身测试。七安装包、五平台清单及总清单核验完成前不创建公开Release。
- 当前正式publish=true流程所需签名配置未在仓库级配置；组织级清单因权限不足未能读取。tools/native/check-release-qualifications.py实际拒绝两个placeholder，此状态保留。
- 当前阶段：v0.6.1已公开且为Latest；后续仅更新发布记录文档，标签仍固定实际构建源。完整CI、产物哈希和发布回读收据保存在本工作树忽略目录.run/release-0.6.1/。
