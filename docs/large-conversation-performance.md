# 大型长期会话加载性能修复报告

## 范围与数据边界

本次修复针对普通打开、切换、恢复和向上分页，不改变 Codex/ACP 的模型上下文、
compaction 或原始 rollout。原始 JSONL 只读，`conversation_id`、external session、
工作目录、模型、分支关系和既有产物记录均保持不变。

当前工程所在 Linux 环境中未发现 conversation 26 的实际 SQLite 数据库、external
session `019eb4a6-3d2f-7923-ac9b-f4c31bf826f4` 对应 rollout，或正在运行的 Codeg
进程。因此，修复前的实际数据采用用户提供的只读诊断结果；修复后使用同一代码路径的
3GB 稀疏 rollout、758 条产物、1261 条映射、753 条 run、14 个分支和 7 条回归记录的
隔离夹具复核。本文不会把夹具结果冒充目标 2.281GB 会话的现场实测。

## 修复前调用链与根因

普通切换的调用链为：

1. `useConversationDetail`/运行时 store 触发 `get_folder_conversation`；
2. Web/Tauri transport 调用会话详情命令；
3. 后端读取 conversation/folder/branch 元数据；
4. Codex parser 定位并解析 rollout JSONL；
5. 关联 delegation、branch merge、artifact、deliverable/run 数据；
6. JSON 序列化、HTTP 压缩、传输和浏览器 `JSON.parse`；
7. store 提交，消息适配，React commit，虚拟列表首帧；
8. ACP 恢复与上述历史加载并行进行。

存在两个会退化为全量解析的兼容入口：

- `tailTurns`/`fromIndex` 进入 `get_folder_conversation_with_live_core`，先完整解析再切片；
- `get_folder_conversation_turns` 对 Codex 同样先完整解析，再按全局 index 切页。

较早客户端、保留在内存中的旧窗口状态或兼容分页调用一旦携带这些参数，普通切换就会
从 `bounded_history=true` 退化为 `false`。用户提供的现场数据与该路径一致：约
20,760–20,881 turns、29.1–32.6 秒解析、30.9–35.8 秒后端总耗时。

同一次切换的重复详情请求由多个独立触发源叠加：首次 effect、viewer 持久化收敛轮询、
turn 完成补偿轮询、产物/文件变更事件和恢复后的状态同步。旧 single-flight 只能合并
完全同时的同键 Promise；请求结束后到达的下一事件会再次读取。请求也没有 AbortSignal，
不同会话快速切换时只能靠提交阶段的 generation 丢弃旧结果，不能停止旧 I/O。

## 新的首屏和分页协议

- 普通 `get_folder_conversation` 固定走 opaque byte cursor，默认 25 个用户轮次（通常约
  50 条用户/助手消息）。无参数也仍是有界首屏。
- `fromIndex` 在普通详情接口上被拒绝；`tailTurns` 只作为弃用兼容信号并转换为有界
  cursor 请求，不再触发全量解析。
- Codex 的旧 index 分页接口被拒绝，前端向上翻页只使用 `history_page.next_cursor`。
- 首屏详情只组合 summary、近期 turns、当前可见 turn 的少量 artifact/deliverable
  关联，以及最多 20 条回归摘要。
- `list_conversation_deliverable_history` 独立分页返回历史产物；不删除或隐藏既有记录。
- `list_conversation_branches` 独立分页投影轻量分支状态，不读取 branch rollout 或
  `snapshot_context`。
- `list_conversation_branch_merges` 独立分页返回最多 512 字符的 summary preview；完整
  summary 通过用户展开后的 opaque deferred reference 获取。
- `list_conversation_output_window` 只接收当前已显示 turn 的 ID、时间和角色，只查询其
  artifact/deliverable 关联，不接收正文且不读取 JSONL。产物和文件事件改用此接口。
- 大型工具输出、图片、reasoning 和回归正文只在明确展开时通过
  `get_deferred_history_content` 读取；opaque reference 包含会话范围和内容校验信息。

## JSONL 字节偏移索引

索引版本为 v2，存放在用户缓存目录
`codeg/codex-history-index-v2/`，每个 rollout 有一个 metadata 文件和一个固定宽度 entry
文件。原始 rollout 不会被修改。

metadata 记录：规范化路径、文件身份（Unix dev/inode，Windows volume/file index）、
文件大小、mtime、已索引末尾 offset、entry 数、未完成行的可恢复状态、饱和状态和少量
精确页边界。每条 17-byte entry 记录 start offset、end offset 和 user/message/turn/
tool/compaction 类型。

首次打开没有索引时，首屏从 EOF 反向扫描，读取硬上限 16MB；随后独立后台线程以
16MB slice 流式建立索引，每 256MB 输出一次无正文进度日志，每个 slice 持久化检查点并
短暂让出 CPU。进程退出、错误或重启后从 `indexed_through_offset` 继续。JSONL 追加时只
索引新增尾部。

文件身份改变、缩短、同长度但 mtime 改变、版本不兼容、metadata/entry 损坏或 entry
越界时索引失效并安全重建。单索引最多 4,000,000 条 entry（约 68MB）；达到上限后标记
`limited` 并对未覆盖区域使用有界扫描，不返回伪命中。索引总缓存上限 1GB，按最近修改
时间淘汰非活动 sidecar。

## 去重、取消、缓存和状态一致性

前端详情键包含 conversation、cursor、page size 和历史选择器。同键并发调用共享一个
flight；每个消费者独立取消，最后一个消费者离开后才取消底层请求。快速切换会 Abort
旧会话的 HTTP fetch；Tauri IPC 无法中断底层调用，但会丢弃迟到结果。每次详情/分页请求
携带 generation，旧请求的成功、错误和 loading 状态均不能覆盖新状态。

后端 parser 对相同文件版本、cursor 和 page size 使用 single-flight；缓存键包含规范化
路径、文件大小、mtime、cursor、page size 和 cwd。客户端断开会设置取消标志，反向扫描
在每个 chunk 检查并停止。页面内存缓存为 16 项/64MB LRU，上限 32MB/项。

前端最近首屏缓存为 8 项、TTL 30 秒。turn/产物/文件状态的权威刷新会显式失效相关首屏
缓存；JSONL 文件版本同时存在于后端缓存键和响应诊断字段中。产物事件只增量刷新输出
关联，不再无条件刷新消息历史。

## 虚拟列表与 ACP 解耦

消息线程继续使用 `virtua`，只挂载视口附近项目。历史行的一元素 source wrapper 以
WeakMap 复用，避免无关 live token 让已结算历史行重新渲染。大型工具、reasoning、图片和
回归正文在折叠/未展开时不进入 Markdown、高亮或图片解码路径。

向上分页通过显式 `olderTurnsPrependEpoch` 驱动虚拟列表 shift，保持滚动锚点；页面按 ID
去重，cursor seam 改变时拒绝拼接旧页。

ACP durable restore 不依赖详情 Promise，和历史首屏并行。输入框可用性由连接身份、
`promptReady` 和工作目录决定，不依赖 `detailLoading`；历史或产物请求失败也不会把已恢复
的 ACP 连接改成初始化失败。已有 restore single-flight 保持不变。

## 性能复核

| 场景 | 修复前现场数据 | 修复后隔离复核 |
| --- | --- | --- |
| 普通详情历史边界 | 偶发 `bounded_history=false` | 强制 cursor page；兼容全量入口被拒绝/转换 |
| rollout 规模 | 2.08GB（较早诊断），现为 2.281GB | 3,306,149,382-byte 稀疏夹具 |
| 首屏 JSONL 读取 | 最坏完整文件 | 3,075,700-byte 页面；扫描和读取均断言 ≤16MB |
| 冷解析 | 29.1–32.6s（全量路径） | 591ms（6 个工具密集轮次） |
| 热切换 | 仍可重复请求 | 内存缓存命中；持久索引重开为 0-byte reverse scan |
| 工具密集首屏 | 压缩后 4.8–8.45MB | 延迟物化后 6 个 512KB 工具输出序列化断言 <128KB |
| 历史产物 | 首屏可受 758/1261 关联拖累 | 独立 25 项页，total=758，响应断言 <1MB |
| 分支/回归 | 14 个分支和完整回归正文可能叠加 | 14 个轻量摘要 + 7 个 preview，查询断言 <1s |

3GB 夹具的 parser detail（延迟物化前）为 3,075,700 bytes，gzip 为 4,483 bytes；生产详情
随后还会把大工具输出替换成 head/tail preview 和 deferred reference。<128KB 的受控首屏在
8Mbps 下纯传输预算约 0.2 秒以内（含常见协议开销仍低于 3 秒目标），但这不是浏览器网络
整链路实测。

由于目标数据库和 2.281GB rollout 未挂载，本次不能提供 conversation 26 的修复后 P95、
Brotli 大小或浏览器首屏实测。新日志已覆盖 request ID、generation、conversation/source、
cursor/page size、bounded 状态、文件版本、index 状态、起止 offset、read/scan bytes、turns、
parser/关联 SQL/序列化/压缩耗时与字节数，以及浏览器 fetch、JSON parse、React commit 和
可交互耗时；在目标机器升级后可直接按 conversation 26 过滤得到真实对比。

日志只记录 ID、范围、计数、字节和耗时，不记录用户正文、提示词、token、Authorization、
Cookie 或 API Key。

## 验证覆盖

自动化测试覆盖 3GB 首屏硬上限、增量续建、损坏重建、取消、持久索引重启、branch
preview、回归正文按需、758/1261/753 高基数数据库夹具、请求 single-flight、消费者取消、
generation 旧响应隔离、产物事件不重载历史、分页拼接、虚拟挂载和滚动锚点。原始 rollout
在损坏重建测试前后以长度和 SHA-256 双重确认未改变。

仍需在持有目标数据的机器上完成一次升级后的只读现场验收：冷切换、热切换、8Mbps
限速、刷新/重连/重启，并核对本文所列结构化日志。这一步不要求归档、删除、裁剪或重建
会话。
