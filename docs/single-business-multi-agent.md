# 单业务对话、多 Agent 执行

CodeG 的业务 ID、标题、工作目录和产物归属保持不变。首次在输入区域切换
Codex / Claude Code / Pi 时，才为这条业务启用共享历史；未切换的历史对话继续使用原有
原生会话及读取路径。全局默认 Agent 仍只影响以后创建的对话。
启用共享业务后保留其标题，由用户主动重命名；各引擎的原生自动标题与业务标题独立。
迟到的原生标题不会覆盖它。空标题仍接受原始用户消息的初始名称，未启用业务及迁移停用
后的旧对话沿用原有自动命名。已被旧版改写且无法可靠还原的自动标题不猜测修复，可手动更名。

## 数据与执行边界

- `conversation`：一个可见的业务，保存当前主引擎的投影。
- `conversation_agent_session`：每业务、每引擎一个独立原生 session，分别保存
  external ID、模式、模型/推理/权限配置及交接游标。原生压缩、writer、认证和私有
  记忆各归其引擎；没有互写 session 文件。
- `business_message`：一份按 sequence 排序的持久业务历史。键同时包含引擎 session、
  原始 external ID 和原生 turn ID；rollover 重用 turn 编号不会覆盖旧消息。
  普通任务的原生 task ID、耗时、完成时间和 usage 原样保留。
  附件来源只固定在 CodeG 的类型化加载引用中，不扫描正文或其他工具元数据。
- `business_agent_switch` / `business_prompt_receipt`：持久切换请求与消息收据。
  原生 rollover 的旧 ID 存入 alias，跨引擎后仍可按需加载原 Codex 图片、reasoning
  和工具输出；前台历史与模型上下文分别处理。

首次归档通过 25 个用户轮次的分页和私有临时 spool 导入，事务提交后才启用共享
历史。后续只捕获原生尾页；共享历史按用户轮次分页，序号、角色和 session 查询均有
索引。界面仍用原有虚拟列表，不通过全量渲染解决滚动问题。

## 切换、恢复与去重

切换 API 先持久记录 request ID 和 expected revision。正在执行时只排队，当前主轮
结束后才准备目标引擎；不会发 cancel，也不会把合法后台子任务当作第二个主轮。
目标引擎必须完成自身 session/new 或 session/load、选择器和身份校验，之后 SQLite
CAS 与进程内连接锁发布新主引擎。准备失败保留原映射和 writer，可在输入区重试。
这里的身份校验核对原生 session ID；协议握手不能证明模型服务的实时登录、配额或
网络一定可用。原生引擎仍可能在首轮报告认证/服务错误，须使用该引擎自身的认证
入口处理，或切回保留的原 session，不会复制另一引擎的凭证。

同一业务的恢复、主轮入队和切换共用锁；不同业务仍可并行。跨浏览器/桌面客户端
通过 SQLite revision 和唯一约束仲裁，旧请求不能覆盖新状态。后台 worker 启动时恢复
queued/preparing 请求。切换写事务在读取幂等收据前声明写锁，避免流式历史回填与
SQLite 读快照升级竞争；锁声明和幂等重试不推进 revision 或更新时间。
前端提交前只重新读取一次状态，使用任务编辑或原生 rollover 推进后的最新 revision；
若另一客户端已经改变主 Agent 或切换请求，则保留其意图并要求显式重试。
后端 CAS 仍仲裁读取之后的竞争，结构化拒绝显示原始说明，不自动重发切换请求。
客户端断开不终止已接纳的切换。此方案针对一个 CodeG 后端
及其多端客户端；不支持多个独立 CodeG 服务同时写同一 SQLite 数据目录。

有后台子任务的原父连接保留来源和权限路由，标记为 retired 后不能接收新主轮，
也不能用迟到退出事件覆盖新主任务状态。没有后台工作时正常回收。

Goal 的启动/继续 RPC 不接受普通 ACP 消息块。切换后若目标引擎尚未收到最新的
业务交接，会拒绝启动/继续，并提示先发送普通消息。一次有界桥及近期摘录成功
接纳后即可使用 Goal；独立的交接边界记录不会把预算外的旧历史标为已交付，也
不会强迫先把整段历史上传。Goal 的自主续轮仍使用其原生上下文；共享状态、记忆
和 Skill 在普通消息接纳时加载，不会仅凭保存文件就进入自主续轮。

用户草稿继续使用现有持久消息队列。交接结束才发送，稳定 message ID 加原始内容
digest 和持久 accepted 收据去重。SQLite 与任意 ACP 引擎之间没有跨进程原子事务：
如果进程恰好在“发送成功、写收据之前”崩溃，reserved 记录会标为交付未确认并保留
原始消息，**不会自动重发**。不能把这种不确定窗口宣称为跨任意协议的 exactly-once。

## 模型实际收到什么

Codex 原生 thread/turn、Claude Code 原生 SDK session 和 Pi 原生 RPC session
不支持相互无损导入。
本实现发送明确标记的上下文桥，不把可见历史冒充原生上下文。

每次普通主轮消息接纳时加载：用户确认的任务状态；global/project/task 范围的共享记忆及
来源、revision、更新时间；目标 session 尚未收到的跨引擎用户/助手正文增量。
增量按时间顺序、有固定条数和文本预算；达到预算时明确提示缺口，保留未交付游标。
另附小型近期边界摘录，避免很长的旧历史挡住最新文件和决定；摘录不确认中间缺口。
原始历史可通过 CodeG 的会话工具按需检索。

不传递原生 thinking、孤立 tool call/result、大图片字节或工具原始输出。图片以文件
引用交接；附件、原工具输出和产物仍在统一历史及产物列表可查。桥只在传输时加入，
前台隐藏注入到 user 记录的桥；助手回答和原始 rollout 不改写。

共享任务状态由用户在“共享任务与记忆”中确认，包含目标、有效要求、已完成、待办、
最新文件和关键决定。共享长期记忆只接受 user 来源并执行 revision 冲突检查；不会把
模型推测、工具错误或临时输出自动固化为事实。清空已有记忆也会保存为空。

项目 Skill 主体保留在 `.agents/skills/<name>/SKILL.md`，不搬动现有工作区或私有
`.codex` / `.claude` / `.pi` 目录。每次只加载目录；用户显式使用 `$name` 或 `/name` 时，
把同一主体在预算内读入当前引擎请求。超过预算明确要求按需读文件，不宣称已经加载。
已有公文/手册模板及验收规则不会被另建模板替换。

## 能力核查及限制

| 能力 | 实际接入与边界 |
| --- | --- |
| Codex | app-server thread/resume、turn/start，经固定 Codex ACP 适配；保持自身 thread 和压缩状态。 |
| Claude Code | Claude Agent SDK 的独立 session，经固定 Claude ACP 适配；自身认证、权限和工具处理。 |
| Pi | 定制 pi-acp 0.0.34-codeg.2 启动独立 `pi --mode rpc`；随包固定 Pi 1.4.0、Node 24.16.0。使用自己的原生 session/load、模型和 thinking level；支持返回 Pi 原对话或从已有 Pi 业务切出。缺失运行时、认证或项目资源信任时，准备失败保留原 Agent，可在自身设置中处理后重试。 |
| Plan | Codex 原生 collaboration mode；Claude 使用其适配器实际广告的原生 plan 模式。Pi 广告的 mode 是 thinking level，不能把它当作 Plan。 |
| Goal | Codex 原生持久 thread/goal RPC。非原生 Goal 扩展不能用提示词代替。切主 Agent 前在轮边界确认暂停；回切不自动继续。暂停成功后目标准备失败，原 Goal 仍保持暂停。 |
| MCP | Codex/Claude 支持 session MCP 传递。定制 Pi 适配器通过 Pi 1.4 原生 `registerMcpServer` 接通 stdio/HTTP MCP 与 companion，支持真实委托和产物登记；SSE 明确拒绝。浏览器/桌面/Office 仍需对应工具和宿主能力，不能仅凭 MCP 连接就宣称可用。 |
| 浏览器 | 沿用 CodeG browser 工具及其当前可用性；绑定原连接的浏览器状态不能作为目标引擎的已有工具状态。 |
| 桌面控制 | 依赖当前宿主及打包 helper，继续遵守原权限。Server 无桌面或宿主不支持时不宣称可用。 |
| Office | 没有协议级的跨引擎 Office 会话迁移。共享文件/Skill 可以复用；外部 Office 工具必须在目标环境独立可用。 |
| 宿主专属工具 | 目标引擎通过自己的发现/调用结果判断可用性；不把上一引擎调用成功当作兼容性证据。 |

核查来源：`src-tauri/src/acp/registry.rs` 的固定适配器与能力描述；Codex 官方
[app-server 文档](https://developers.openai.com/codex/app-server)；Claude 官方
[headless 文档](https://code.claude.com/docs/en/headless)；ACP
[session/load](https://agentclientprotocol.com/protocol/session-setup)。具体版本以发布包和
CI 安装的固定 ACP 版本为准。

业务分支继续独立创建、恢复和回归。跨引擎业务分支使用明确标注的有界 snapshot；
只有一个引擎的 native fork 无法代表全部业务上下文。显示历史仍分页继承冻结边界。

## 迁移与验证

`m20261025_000001_business_agent_sessions` 仅增加表和索引，不批量改写历史对话。
其 `down` 恢复首次启用时的原 Agent/external/model，停用映射和排队请求；新增历史、
session、收据及记忆作为停用归档保留，不 drop 用户数据。回滚须离线备份数据库、
停止使用这份数据目录的服务后，由迁移 runner 执行 down；不要直接用旧版进程与新版
并发操作同一数据目录。重新启用前应审查保留归档的恢复范围。

自动化覆盖双引擎交接块和去重、运行中排队、错误身份回退、跨端 CAS、快速重复请求、
SQLite 关闭重开、独立参数、消息收据、记忆/Skill 实际进入 prompt、分页、旧任务耗时、
rollover 重号、迟到解析、子任务父连接和业务分支。
Pi 扩展覆盖三份独立原生身份、Codex→Pi→Claude→Pi→Codex 增量交接、Pi 原业务切出、
Pi 配置与历史在重启后保持、排队和错误目标回退。Pi 映射使用原有 agent_type 文本列和每引擎唯一索引，不移动原会话文件。外部投递另加
`m20261026_000001_business_delivery` 持久队列/收据迁移，其 down 保留记录并暂停未完成项。

Windows 发布验证在一次性 runner 的独立目录安装旧 Setup、升级新版，再分别运行
Setup WebView2 与 Server Web。真实 ACP/原生运行时连接本地确定性模型端点，验证
Codex→Claude→Codex、Codex↔Pi、Claude↔Pi 的实际模型请求、Pi 运行时缺失后的
回退与重试、轮边界排队、同一可见业务、刷新、Pi 原生进程重启恢复和
其他业务隔离；继续执行耗时、总产物、原生 Plan/Goal 和长列表滚动回归。确定性端点
用于证明协议与传输，不证明模型推理质量或真实账号认证。

当前工作环境的 Codex 未登录，不能宣称真实账号下的双引擎推理往返已通过。最终结果
以本次保存的测试日志、CI 和重新下载的发布资产校验为准。

外部消息投递、旧连接兼容与崩溃窗口见[稳定业务投递接口](business-message-api.md)。
Pi 身份映射、原生 MCP、分支、配套运行组件与 Fast / Speed 的实际语义见
[Pi 运行组件与服务档位](pi-runtime-and-service-tier.md)。
