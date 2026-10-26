# Agent 原请求附件接收：实现与验收

2026年10月27日，`codex/agent-file-transfer`。A2 接收部分接入当前 Codex
运行链；发送、投递状态和真实账户/平台验收继续在范围内，A2 尚未完成。
依赖 Server PR7 的原请求附件元数据接口。

## 运行行为

当前 SDK 支持认证附件下载、Server 提供 `agent-attachment-metadata-v1`，且
主人已显式启用编程执行或 Room 文件工具时，macOS/Linux 执行端才使用
`PollFiles`。其余情况保留原行为。附加能力检查每30秒更新；实际工具操作仍
逐次检查原 owner、设备、租约、binding generation 和当前 Matrix 权限。

动态工具 `list_received_files` 只列出本次原始事件的声明；`receive_file` 只
接受该事件的 event_id，拒绝其他事件、Room、URL、key 和目标路径参数。
模型列表、审批提案、日志 Debug 以及工具结果都不带私有 MXC/解密 key。
声明文件名、类型、大小不作为文件内容或落盘路径。主人请求拒绝策略生效后，
缓存的声明也不能继续返回。

接收执行三层 high-risk 策略和原提案审批，后续仍复核原执行与本地策略。
生成路径位于选定工作目录的 `.hagency-received/binding_<digest>/`，文件名
由当前 scope、dispatch、execution、event、声明摘要和工作目录派生。
路径不含发送者提供的名字。目录0700、文件0600；拒绝符号链接、非普通文件、
硬链接和目录替换。保留实际目录对象，写前后检查工作目录和父目录身份。

SDK 使用当前账号/epoch、既有 HTTP/TLS 客户端和认证 media endpoint；不读共享
预览缓存，不调用先整包缓冲的 SDK 下载 API。总下载期限30秒、最多4个下载，
单文件8MiB；检查响应声明及分块长度，v2附件经 SDK密文摘要校验/解密。
原声明大小若存在必须匹配实际字节。实际明文字节数/SHA-256写入成功回执。

## 原始写入所有权与恢复

本地主人账本先提交 reserved，只有首次插入能取得不可克隆的接收所有权。
写目录或 staging 文件前提交 write_possible；随后完整写入、同步文件、原子
禁止覆盖的 rename、同步父目录和精确对象/字节读回，最后保存 ready 回执。

下载或授权在文件写入前已知失败，可以保存 failed 并释放字节预留。取消、
丢失返回、重启或恢复快照，不能从 reserved/write_possible 重新创建所有权。
相同附件的再次调用只在 ready 的原对象、路径、权限、字节和摘要仍匹配且
授权当前有效时返回；不因“内容一样”覆盖另一文件或补造成功回执。

全局保留最多4096条记录，按接收内容字节登记128MiB额度；未知写入按8MiB保留。
这不是通用工作目录的磁盘配额。
旧版账本快照迁移空表，新快照保留这些状态，不能恢复成可重试意图。
账本快照不携带工作目录文件；恢复后的历史 ready 不保证新设备已有文件。

审批审计记录真实完成/失败/未知。接收能力有单独的模型上下文标识，沿用
相同的费用作用域；结算只接受精确已知格式，未知用量同时清除当前接收上下文
基线，不能继续使用旧累计量或清除其他作用域的基线。

## 实测与限制

实际磁盘测试覆盖字节、明文哈希、权限、原子禁止覆盖、审批缺失、错误执行/
事件、策略撤销、符号链接、工作目录替换、丢失所有权、已知回执重校验和快照
恢复。动态工具通过原提案 owner queue 测试批准/拒绝、重复调用只下载一次、
下载中撤权不落盘，以及元数据缓存的当前策略拒绝。

实际 Matrix SDK 与本地 HTTP 服务验证认证头、纯文件字节、真正的附件加密/
解密、篡改拒绝、响应/分块超限、旧服务器能力拒绝和账号切换；无生产消息。
当前 NativeOwner OAuth/设备/TCP/账本/工作目录链的独立测试验证文件轮询、
工具目录、模型实际读回5字节、租约续期和最终回复。后者使用明确的假 provider
及合成 SDK 字节源、显式 AllowWithRules 策略，不能当作真实模型、手工主人审批
或真实 homeserver 联合验收。真实 SDK下载由上述独立测试覆盖。

最终检查结果记录在本文件末尾。现有忽略项仍是忽略，先前双 Agent 真实资格
记录占位失败也不因本批接收测试而通过。

最初整链在接收审计完成后转 unknown。独立回归证明结算拒绝了新增接收能力
的上下文格式；已修复精确格式校验与用量缺口基线处理。原失败保留在
`/tmp/hagency-receive-native*.log`、`/tmp/hagency-receive-accounting.log`。
修复后的首次库回归通过了文件与回复，但旧公共 fixture 的续租断言失败：
该新假模型过早结束，未覆盖既定续租间隔。现让该测试任务继续运行6秒，保持
原续租断言与20秒上限；原记录是 `/tmp/hagency-receive-libraries-pre-heartbeat.log`。
没有放宽授权、断言或未知重试规则。

安装的 Codex0.164.0-alpha.11.1 的实验性 JSON schema确认动态工具目录及调用/
响应字段；导出目录 `/tmp/hagency-receive-codex-schema-experimental`。这仅验证
协议字段，尚非真实模型调用接收工具的验收。

后续仍须完成 send_file/get_file_delivery、真实联合路径、桌面人工与平台验收，
以及 A4 加密 Agent Room/远程审批、A3 协作和前几阶段剩余资格门槛。当前磁盘
发布实现是 macOS/Linux，Windows没有因此获得接收可用声明。


## 最终本机结果和远端门槛

最终 Agent-local125通过/3个原有忽略、engine256通过/7个原有忽略、App618通过/
2个原有忽略，programming3及runtime8通过。三包严格all-targets no-deps Clippy、
核心两包格式、新SDK下载模块格式、production caller及规格绑定检查通过。
日志为 `/tmp/hagency-receive-release-{libraries,runtime,clippy,fmt}.log`、
`/tmp/hagency-receive-{sdk-fmt-check,callers,bindings}.log`。

整个App的既有格式漂移仍使 `cargo fmt -p hagency-app -- --check` 失败；例如未
修改的 article_comparison_native.rs 在 HEAD 单独重现。未将数千处无关格式变化
混入本批。完整输出和基线复现为 `/tmp/hagency-receive-app-fmt-check.log` 与
`/tmp/hagency-receive-app-baseline-fmt.log`，新SDK模块的独立格式检查通过。

上批 metadata 提交0f28ea9a的Client CI与配套Server CI已通过；全矩阵的Mac任务
在规格绑定步骤被Node的600000ms cargo --list上限SIGTERM，不是成功。原日志
保存在 `/tmp/hagency-pr11-macos-0f28ea9a.log`。日志显示冷编译/链接尚在进行；
本批把完全相同包和feature范围的 --no-run 编译放到独立CI构建步骤，保持规格
守卫原10分钟上限、选择器和断言。新远端结果仍待验证，旧失败不改写。
