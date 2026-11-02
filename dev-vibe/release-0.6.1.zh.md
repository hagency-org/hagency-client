# Hagency Client 0.6.1 发布记录

- 用户明确选择沿用0.6.0的未签名、手动安装发布方式，公开说明缺少正式签名和两项真实环境验收；不把占位验收或跳过的发布检查记为通过。
- 基于main091931f21，版本同步至workspace、App、21个本地包锁项、macOS bundle和占位记录的版本pin；第三方依赖零变更。
- 发布前执行现有app-release.yml的publish=false构建，保留三平台engine和五平台安装包自身测试。七安装包、五平台清单及总清单核验完成前不创建公开Release。
- 当前正式publish=true流程所需签名配置未在仓库级配置；组织级清单因权限不足未能读取。tools/native/check-release-qualifications.py实际拒绝两个placeholder，此状态保留。
- 当前阶段：准备源码和构建；尚未发布、尚未建立v0.6.1标签。实际CI、产物哈希和发布回读收据保存在本工作树忽略目录.run/release-0.6.1/。
