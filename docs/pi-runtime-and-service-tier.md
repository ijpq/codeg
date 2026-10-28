# Pi 运行组件与 Fast / Speed

本次固定 CodeG `0.40.0-fix6`、定制 `pi-acp 0.0.34-codeg.2`、Pi `1.4.0` 和 Node
`24.16.0`。适配器源码 commit、tarball SHA256 与依赖锁保存在 `vendor/pi-runtime/`。
Server ZIP 和 Setup 都携带私有 `pi-runtime`，无需全局 npm 或开发机 PATH。上游 MIT
许可证、Pi 及依赖许可证和 Node LICENSE 随运行组保留。

## 运行与迁移

CodeG 优先使用程序目录下经 `source.json` 版本核对的适配器及 Pi。构建/发布验证逐文件
检查 `runtime-files.json`，启动记录实际路径、Pi/ACP 版本，ACP initialize 返回自身版本。
`CODEG_PI_ACP_PATH` 可显式选择自定义适配器；`PI_ACP_PI_COMMAND` 可显式选择 Pi。
不会安装或覆盖已有共享 CLI；自定义版本必须广告 CodeG Pi v1 的身份/MCP 能力并通过
原生 bridge/identity 校验，否则给出升级原因，不静默用全局旧版接替。整组升级/回滚
替换应用目录的运行组件；用户认证、模型、Skill、扩展、工作区及原生历史留在原处。

回滚采用停止自有实例后恢复升级前离线数据库快照和对应整组安装包的方式；保留升级后数据
另行归档，不让旧版迁移器直接打开新版数据库。本次隔离 Windows 验证会重新安装旧包，
核对主程序/MCP 字节并以升级前数据启动、读取历史，同时保留升级后的数据目录。

新建会话先由 Pi 初始化专用空文件，ready 前核对 `get_state`、真实 JSONL header、
workspace 和路径。ACP session ID 可以是旧别名，显式映射到独立原生 ID 和文件；
不是同一个 ID 时也不改写 header。恢复旧 map 时只根据其明确文件指针和真实 header
修复别名，不凭文件名猜测。缺文件、冲突或无映射返回错误，不偷偷新建并沿用旧绑定。
空会话在首次发送前已经持久化，初始化失败保留文件，迟到恢复不能覆盖成功的新状态。

设置页的版本和升级判断使用实际选中的 ACP 路径及其包元数据，不再借用全局 `pi-acp` 的版本。
随包或自定义 ACP 不提供全局 npm 覆盖/卸载动作；随包组件通过匹配 CodeG 版本整组更新。
未知自定义包装器的版本标为未知，不伪装探测成功。

`_pi/identity.runtime` 查询实际用于 RPC 的可执行路径及有界探测的版本。查询失败标为
未验证，不借用全局 Pi 的版本或变更日志；定制版移除了全局 npm 升级提示。

定制适配器经 Pi 1.4 原生 `registerMcpServer` 连接 ACP stdio/HTTP MCP；SSE 明确不支持。
CodeG companion 提供真实异步子任务与 `publish_deliverables`，仍受现有权限及任务绑定
约束。没有宿主浏览器/桌面/Office 工具时不因为 MCP 握手成功就宣称可用。Pi 原生
clone 支持当前尾部 fork，源 writer 不变；任意旧 fork point 和多引擎业务使用 CodeG
明确标注的 snapshot fallback，分支与回归沿用持久幂等机制。

## Fast 的实际语义

核查发布使用的 Codex ACP 2.2.2：CodeG 现有 `fast-mode` 来自 ACP 广告的配置项，
`resolveFastServiceTier` 在开启且模型支持时向原生 app-server `turn/start` 发送
`serviceTier:"fast"`，关闭发送 `null`。不能直接把这个 UI 开关解释为 CodeG 向所有
引擎写入 `service_tier=priority`。原生 Codex 自行决定模型与接口的支持情况。

Pi Responses SDK 有 `serviceTier → service_tier` 传参能力，但 Pi RPC 无原生 tier setter。
本次在定制 ACP 声明独立 `service_tier` 配置项，通过随包原生扩展的
`before_provider_request` 修改真正请求：关闭发送 `default`，开启发送 `priority`。
推理强度保持独立。只对 GPT/o 系列且使用 `openai-responses` 或
`openai-codex-responses` 的已识别模型开放；其他接口/模型显示简短不可用原因并拒绝设置。
这表示支持请求该档位，不保证账号、模型或中转允许使用；服务拒绝时原样失败，不删参数
重试、不换模型、不降推理强度。

设置默认关闭，按独立 session 及 provider/model/API/endpoint 保存。原生扩展命令用 nonce
确认落盘，不插入用户消息。当前主任务执行期间拒绝修改模型、thinking 和档位；空闲时
修改对下一轮有效。切回 Agent 或模型恢复它自己的设置。外部投递固定接收时配置，排队后
配置改变明确失败，详见[外部投递契约](business-message-api.md)。

请求值与确认值分开记录：只读原始 `response.completed.response.service_tier`，不用 Pi
usage/pricing 的请求值兜底冒充实际使用值。没有返回保持“已请求／上游未验证”；中转返回
仅标为 endpoint reported，上游仍未验证；直接官方 HTTPS 端点返回才标为 upstream
reported。返回与请求不同的档位照实显示，不推断速度。脱敏观察仅含模型摘要、档位、
验证类型、HTTP 状态及时间，不记录 token、URL 凭据、正文或认证。

选项提示可能增加费用或额度，具体由当前供应商/中转决定，没有套用官方倍率或自动开启。
本次隔离端点能证明参数、拒绝、缺返回值及改写处理；没有用用户真实中转发起计费请求，
因此不声称该中转转发、上游实际档位或加速收益已验证。

官方语义参考：[Codex speed](https://developers.openai.com/codex/speed)、
[Responses create 的 service_tier](https://developers.openai.com/api/reference/resources/responses/methods/create)。
官方接受的别名和计费描述不等于任意中转的合同。
