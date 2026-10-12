# 超长 Codex 会话恢复：诊断与边界

本次基线为 upstream v0.35.1（e21eb6b38eea9405cdfa3b2d99df216d101d1344）。
生产机器“2k自动化”的 4.44 GiB、273 秒总恢复、2.3 秒握手、268 秒 resume、
约 1 秒连接复用来自用户只读诊断。本工作环境不能访问该 Windows 机器、rollout 或
`LOCAL_CUSTOMIZATIONS.md`，没有把合成测试结果当作该原会话的实测结果。

## 分层证据

- CodeG 的 `acp_restore_conversation` 已有完整请求 single-flight 和就绪连接复用。
  服务端恢复日志分别记录配置准备、进程/ACP 就绪、外部 session、prompt-ready 和发布绑定。
- 侧栏预览被另一预览替代时，前端 `disconnectIfIdle` 原来会调用真正的
  `acpDisconnect`，结束空闲 owner 的 CLI。正常组件卸载则只释放 surface。这使回到
  同一个长会话也可能重新冷恢复。现统一释放 surface，保留服务端连接及既有 idle GC。
  活动任务、显式断开、重配配置、取消、断线补偿和前端事件代际保护保持原有语义。
- 锁定的 `@agentclientprotocol/codex-acp 2.1.1` 已向 CLI 发送
  `thread/resume { excludeTurns: true }`。ACP 的请求经过 CLI 后才完成；无需返回完整 UI
  turns，仍可能耗时很长。恢复时的真实请求和响应、耗时和进程 ID 由诊断代理记录，
  不记录模型提示或认证内容。
- 此适配器实际默认解析到内嵌 CLI 0.159.3，不能以全局 `codex --version` 代替它。
  官方 CLI 0.162.1 的源码及文件级 strace 证实：legacy JSONL 在 app-server 读完整
  initial history。配置加载会释放协调锁，再重新检查会话和历史；没有请求内复用时，
  第二次检查又读一遍。缺少可复用的 revision 时，live writer 建立写入锁后也可能再次
  读取 model context。首次安装的 SQLite 元数据回填还可能在 initialize 阶段再读一次。
  原生 paginated history 已有检查点倒序读取；legacy 路径没有同等恢复优化。
  实际默认 CLI 0.159.3 的 64 MiB 文件 trace 同样测得冷 resume 读取
  134,431,825 字节（约两遍完整文件），复用只读取 24,576 字节。未修复 CLI 作为负对照
  会被新增的有界读取验收断言拒绝；它没有被算作优化候选通过。
- `scripts/diagnose-codex-resume-layers.py` 连接真实 ACP 和 CLI，并可连接隔离 CodeG
  Server。`scripts/test-codex-resume.py` 比较同一官方 CLI 版本的完整恢复与补丁恢复。
  两者使用独立数据目录、随机回环端口和本地 Responses 测试服务，不访问付费模型。

64 MiB、元数据已建立的真实 CodeG→ACP 2.1.1→CLI 0.159.3 合成基线：CodeG
首次恢复 3031 ms，后续复用 68 ms；ACP 单独测量握手 712 ms、resume 808 ms、
复用 40 ms。并发再次打开返回相同连接，没有重新执行 resume 或调用模型。
这些是新进程的冷恢复，不等同于清空操作系统文件缓存后的磁盘测试。

## 最小修复的责任边界

CodeG 修复非必要断开；历史读取优化在 CLI 内实施，不以减少前端消息、检测最终回答、
改变 external_id、注入 `thread/resume.history`、创建影子 session 或加长超时替代。
Windows Server ZIP 携带私有、可复现 CLI `0.162.1-codeg.1`。其官方源 tag、精确 commit、
补丁 SHA256、许可证和严格 64 MiB 探测上限记录于 `vendor/codex-runtime/`。
Windows 构建沿用原生包脚本校验并获取 Codex 自建的 sandbox-enabled V8，与 code-mode
host、sandbox setup/service/runner 和 ripgrep 一起发布，未改动 V8 特性或依赖版本。
默认 Setup 保留上游的 CLI 打包机制，连接复用修复适用于两种运行模式；私有 CLI
恢复优化由 Server ZIP 的 `runtime/` 提供。

CodeG 只有发现与本次编译 manifest 完全匹配的 `runtime/source.json` 和程序时才选择
该运行时。用户设置或继承的 `CODEX_PATH` 优先；`CODEG_USE_BUNDLED_CODEX=0` 可禁用
选择；`CODEG_LEGACY_CHECKPOINT_RESUME=0` 保留官方恢复路径。不会修改全局 CLI、
用户配置或正在运行的生产服务。部署 Server 必须使用完整包。

CLI 改动仅向既有 `InitialHistory::Resumed` 提供已验证 history 和 revision，继续使用
原 session、rollout 路径和原生 writer 锁。没有新的持久检查点缓存或历史副本。

1. 只为 `excludeTurns=true`、未请求 initial turns page、未指定自定义 path 的恢复启用。
   UI 历史读取、fork、分页及其他调用方默认不启用 legacy 检查点优化。
2. 倒序探测最多读取 64 MiB。候选必须是最新原生 compaction，有 replacement history、
   window number、resume metadata，并有其后的 TurnContext 和 full WorldState。
3. 使用 CLI 原有用户轮次判定函数和运行时版本选择函数校验，且必须有原 session 自己
   的 ThreadSettingsApplied 快照及匹配 checkpoint turn ID 的结束事件，并通过 CLI 原生
   snapshot 的未完成任务判定。缺少这些状态、
   checkpoint 在 EOF 而不能证明首次轮次语义、未完成任务或 settings 属于其他 session
   时，安全回退完整读取。不会猜测摘要文本、伪造用户消息或猜测任务已完成。
4. rollback、损坏尾部、超预算单条记录、没有/旧/无效 checkpoint 都回退原生完整读取。
   不会跳过超大记录后将较旧状态当作最新。
5. 每次读取前后核对真实路径、文件大小、修改/创建时间和文件标识；Unix 使用 dev/inode/
   ctime，Windows 使用 volume serial/file ID。取得 writer 锁后再次验证。文件追加、
   同名替换或标识不可用会失效；失效后的 writer 直接完整读取，不消费未验证的后缀。
6. 完整回退也携带稳定 revision。异步配置加载后仍查询当前会话、归档/删除状态和配置
   状态；只在同一个恢复请求中、原 session/path/file identity/size/time 均不变时，复用
   首次读取的历史。native writer 在写入锁下再次验证，变化时仍重新完整读取。
   旧检查点因此可避免配置重检及 writer 阶段的重复全量读取；首次元数据回填和必要的
   一次完整恢复仍可很慢。没有把不完整检查点改成可用，也没有跨请求历史缓存。

## 完整性和回归测试

原生 core 回归比较 checkpoint 与完整 replay 的完整重建结果（窗口、归因、前轮设置、
历史与工具），补充首次轮次、运行时、owner settings、未完成任务的拒绝用例。
thread-store 回归覆盖严格读取预算、完整 checkpoint、旧摘要、缺少 companions、坏尾部、
追加及等长等 mtime 文件替换；Windows 额外验证 volume/file ID。
配置阶段重复读取的新断言已在修复前的诊断二进制上实际失败，准确报出全量重复读取；
旧检查点和有效检查点使用不同的严格读取上限，不以放宽检查点验收覆盖旧格式。

协议回归比对实际发往模型的输入，保留工具 call ID、输出与图片实际内容。只归一化新轮
生成的消息 ID 和隔离 CODEX_HOME 路径。原文件初始前缀逐块 SHA256 校验，允许 CLI 正常
追加新轮，禁止重写原前缀。覆盖冷/复用/重启、缺失和无效检查点、缺少 runtime/settings、
超 64 MiB 尾记录、坏尾、追加、替换和模型输入等价性。图片在当前有效原生上下文中保留，
compaction 前图片仍在原 rollout 中供原生历史按需读取，不通过删除图片降低读取量。

大文件合成样本使用密集可丢弃 JSON 扩展字段，避免把几 GiB 全部当作模型内容塞入 RAM。
这可以精确检验实际扫描字节和读取次数，但不是用户真实图像/文字/工具分布的复制。
最终性能数字及完整测试/Release 证据另存交付报告；不承诺合成文件耗时等于生产机器耗时。

## 最终合成性能（Linux aarch64）

日期 2026-10-12。文件初始前缀 4,769,680,096 字节，约 4.44 GiB；使用密集的
可忽略 JSON 扩展字段、真实 PNG 与工具配对。SQLite 元数据已建立。测量为新进程冷恢复，
不是操作系统文件缓存冷读。4.44 GiB 对照使用同一个私有诊断二进制，通过关闭/开启
`CODEG_LEGACY_CHECKPOINT_RESUME` 比较；诊断 CLI entrypoint 使用 opt-level=0，依赖仍
为 release，Windows 正式包按 release/thin LTO 编译。并行编译负载会影响耗时，读取字节
来自 strace，不用进程 IO 计数估算。

| 场景 | 原生路径 resume | 修复路径 resume | 原生读取字节 | 修复读取字节 |
| --- | ---: | ---: | ---: | ---: |
| 有效原生检查点，冷恢复 | 62.531 s | 0.248 s | 9,539,540,417 | 229,377 |
| 有效原生检查点，进程重启 | 58.722 s | 0.381 s | 9,539,614,009 | 229,377 |
| 旧检查点，完整回退 | 64.283 s | 38.389 s | 9,539,540,091 | 4,769,909,310 |
| 旧检查点，进程重启 | 54.300 s | 47.120 s | 9,539,613,837 | 4,769,946,183 |

有效检查点冷恢复的完整进程树峰值 RSS 为 479,180 / 444,252 KiB（原生/修复），
同一连接再次恢复 12.081 / 12.400 ms，只读取 24,576 字节。旧检查点完整回退峰值
462,516 / 429,276 KiB；后续复用 19.500 / 460.381 ms。两种 4.44 GiB 测试均逐块
校验原 rollout 前缀，原 session ID 和实际模型上下文一致。没有通过裁减 UI 获得这些结果。

64 MiB 官方 0.162.1 与私有 CLI 对照测得冷恢复 874.539 / 269.907 ms；其请求内历史
复用回归在旧二进制上确实失败，报出重复完整读取。缺失检查点和超预算记录的倒序探测
本身最多读 64 MiB，之后执行一次必要完整恢复，不能把这一有界探测也描述为零读取。

真实 ACP 2.1.1 + 修复 CLI：握手 310.084 ms、resume 145.874 ms、再次恢复
34.101 ms；恢复后提问 118.390 ms，收到 end_turn，只有一次模型请求，工具/图片/上下文
完整。隔离 CodeG Server 冷恢复 989.736 ms、复用 22.224 ms，并发再次打开仍只有一次
CLI resume。ACP 2.2.2 对照也通过：CodeG 冷恢复 894.317 ms、复用 25.260 ms，
并发再次打开 24.655 / 37.931 ms，相同连接和外部 session；恢复阶段没有模型请求。
ACP 单独恢复耗时受同时进行的大文件测试影响，记录握手 316.496 ms、resume
1891.377 ms、复用 47.569 ms，恢复后提问 615.722 ms，协议与完整性断言全部通过。

原始 JSON、RPC 时间线、strace、服务器日志和失败负对照保留在
`artifacts/codex-resume-20261012/`，正式 Git 提交不包含临时数据或合成用户目录。

## 已执行检查及限制

模型输入比较先归一化实际字符串值，再编码 JSON；Windows 目录中的反斜杠不会因 JSON
转义导致假差异。3 项跨平台比较测试保留旧逻辑失败的负对照，图片、工具与上下文变化
断言保持完整。已有 6 份实际模型输入的 Linux 对照结果完全相同。

前端 625 个文件、9100 项测试通过；CodeG Rust 5106 项 library 与 2 项 server
测试通过，保留 1 项既有 ignored。TypeScript、lint、静态导出、浏览器 bundle/type
检查、CodeG cargo check 和严格 clippy 通过。CLI thread-store 278 项、core 专项
2 项、app-server 原有 thread_resume 集成 69 项通过；core/store 严格 clippy 通过。
AIR/terminal 31 项、Setup 校验 6 项、draft-release 回归 7 项通过。

app-server 原生测试线程使用官方 nextest CI 同样的 `RUST_MIN_STACK=8388608`；初次
默认 2 MiB 测试线程栈溢出的日志保留。没有改动生产栈、生产超时或测试断言。
Windows CI 必须执行原生恢复回归、正常 release 构建、完整 helper 布局、实际版本及
隔离启动；正式 Release 之后还要重新下载、SHA256、解压和 Windows 安装/启动验证。

生产“2k自动化”尚未现场验证。若原会话没有完整的原生检查点，只能安全完成一次全量
恢复；首次元数据回填和 180 秒 idle GC 之后的真正冷恢复仍可能慢。显式 `CODEX_PATH`
覆盖也会让 Server 不使用随包的优化 CLI。旧图片保留在原 rollout，通过现有历史按需
读取；有效压缩后的模型上下文与原生完整 replay 一致，不把压缩前旧图片重新塞入模型。
