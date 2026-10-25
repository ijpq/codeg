# 外部程序向业务对话投递

本次只修改 CodeG，没有检查或修改 2K 等调用方。接口沿用现有 Server token 和权限；
新增端点没有免认证入口。桌面端通过同名 Tauri command 调用共享核心。

## 稳定契约

向 `POST /api/submit_business_message` 提交 JSON（也接受 `{ "request": {...} }`）：

```json
{
  "conversationId": 123,
  "clientMessageId": "caller-generated-stable-id",
  "requiredAgent": "pi",
  "mode": "queue",
  "deadlineSeconds": 3600,
  "blocks": [{ "type": "text", "text": "整理会议议程" }],
  "metadata": { "source": "external-program" }
}
```

`conversation_id`、`client_message_id`、`required_agent` 和 `deadline_seconds` 也接受
下划线写法。`conversationId` 必须是 CodeG 数据库的业务 ID，不能是原生 session ID。
调用方不保存 connection ID；已有持久 session 的恢复、ready 等待和投递由后端负责。
尚未绑定原生 session 的空白对话明确报错，需先在 CodeG 初始化；接口不会猜测或创建
另一个可见对话。初始化和恢复均有超时，失败不被当作接收成功。

附件使用与 `acp_prompt` 相同的 `PromptInputBlock`：`image` 含 `data`（base64）、
`mime_type`、可选 `uri`；`resource` 含 `uri` 和可选 `mime_type/text/blob`；
`resource_link` 含 `uri/name` 和可选 `mime_type/description`。附件须遵循目标 Agent
实际广告的能力和既有文件权限；不转换成不完整的 tool call/result。沿用 HTTP 请求
体限制，metadata 最多 16 项，每项 key 128 字节、value 2048 字节。

接收时将当前 Agent、原生 session、成功切换代次以及模型、推理、档位等选择值固定到
持久收据，返回 `id/targetAgent/targetExternalId/targetGeneration/targetSettings/status`。
`requiredAgent` 不匹配返回 `required_agent_mismatch`；切换 queued/preparing 时返回
`business_switch_pending`，不占用幂等键，调用方可稍后原样重试。不会擅自切换 Agent。
执行前若目标、切换代次或配置已变化，收据明确失败（`DELIVERY_TARGET_CHANGED` 或
`DELIVERY_SETTINGS_CHANGED`），不改投，不自动切回旧 Agent，不悄悄使用新的档位。

`mode=queue` 在当前主轮之后执行一个普通主轮。`mode=steer` 只调用当前主轮的原生
引导能力；没有活动主轮或不支持时明确失败。Pi 的引导调用原生 `steer`，
`/steering` 仅设置原生队列策略，不能作为消息注入成功的证据。Pi 原生 `follow_up`
与 steer 分开；本统一接口暂不提供 follow-up，提交它返回 `delivery_mode_unsupported`。
引导收据跟踪所属整轮的结束状态，不宣称模型必然采用了指导内容。不能单独撤回已接纳
的 steer；需明确取消所属主任务，避免一次引导取消误伤其他任务。

## 幂等、状态与恢复

同一业务和 `clientMessageId` 唯一。同一内容重试返回原收据，内容不同返回
`idempotency_key_reused`，包括超时后重试、跨端同时提交。幂等键不以 `codeg:` 开头，
长度 1–128；执行失败后若要主动重做，必须使用新键。`POST /api/get_business_message`
以 `{ "conversationId":123, "messageId":"返回的 id" }` 查询；也接受客户端 ID，
冲突时优先收据 ID，建议始终使用返回的 `id`。

状态依次可能为 `queued → delivering → delivered → running → completed/failed/cancelled`。
接收成功不等于 CLI 接纳，更不等于执行成功。返回 `taskId` 后可与 CodeG 整轮任务关联。
`deadlineSeconds` 是接收至任务完成的总期限（1–86400 秒）；到期的排队项失败，活动普通
任务请求取消，steer 超时只结束自己的收据，不取消所属任务。

`POST /api/cancel_business_message` 使用相同查询字段；排队消息持久取消，活动普通
任务仅在 task/connection 仍属于该收据时请求取消。旧 worker 不能覆盖终态或取消结果。
后端重启后恢复队列并以持久任务状态对账；若崩溃发生在原生接纳与收据写入之间，返回
`uncertain/DELIVERY_OUTCOME_UNKNOWN`，不自动重发。原生协议没有跨 SQLite 的事务，
不能承诺任意崩溃窗口的 exactly-once。管理员应查阅对应原生历史再决定是否以新键重做。

新增迁移 `m20261026_000001_business_delivery` 仅新增表和索引；down 将非终态收据标为
`suspended` 并保留历史、请求及收据。整组回滚需离线备份数据库，不能新旧进程并发写同一
目录。升级后 suspended 项需人工审查，不会恢复成自动执行。

## 旧调用

`acp_prompt`、原生连接 API 继续保留；持有当前有效连接、Agent 和 conversation 绑定的
旧调用可继续使用。已 retired、Agent/session 错配和正在切换的连接会被拒绝，不会被重定向
到新 Agent。旧接口不是持久外部队列，断线重试和过期 connection ID 的调用方应迁移到上述
接口。旧接口不自动获得新收据语义。没有运行检查的外围程序不在本次兼容性结论内。
