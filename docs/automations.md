# 自动化(Automations)—— 使用方法与原理

「自动化」是 codeg 里**用户直接操作**的一等功能:把「**一个 agent + 一段任务(prompt)+ 在哪跑 + 何时跑**」配置好并交付运行——也就是「部署一个 agent 去执行任务」。定时到点或手动触发时,codeg 会拉起该 agent、在指定工作目录里执行任务、并回收结果。入口在侧边栏「自动化」。

本文分两部分:**使用方法**(怎么用)和**原理**(底层怎么跑),便于排障与二次开发。

---

## 一、使用方法

### 1. 新建 / 编辑一个自动化

在自动化页面点「新建」,或点已有条目「编辑」,填写:

- **名称**:便于识别。
- **Agent**:用哪个智能体执行(Claude Code / Codex / …)。
- **任务(prompt)**:富文本编辑器,支持 `@` 引用文件、`/` 命令、`$` skill,和聊天输入框一致。**注意**:每次运行都是**冷启动的新会话**——agent 看不到你的聊天历史,任务描述需自包含。
- **目标文件夹(folder)**:任务在哪个工作区文件夹里运行。
- **运行方式(隔离)**——二选一:
  - **在文件夹内运行(shared_in_root)**:直接用该文件夹的工作树;可选指定一个**分支**(运行前在根树上 checkout)。⚠️ 会与你未提交的改动、以及同一根目录上的其它并发运行相互影响(引擎会对同一根目录串行化)。
  - **每次运行新建 worktree(worktree_per_run)**:每次运行在 `automation/<id>/run-<id>` 下开一个独立 git worktree,互不干扰,最干净。
- **触发方式**——二选一:
  - **定时(schedule)**:选 cron 预设,或点「打开计划生成器」用可视化编辑器;原始 cron 表达式收在「**高级**」里。下方实时预览「下次运行时间」。时区自动取本机(只读,但参与下次运行计算)。
  - **手动(manual)**:不定时,只能手动「Run now」。

保存即写库并生效(定时项会被调度引擎接管)。

### 2. 运行、查看、取消

- **Run now**:在详情页 / 条目菜单手动触发一次(触发方式记为 `manual`)。
- **运行历史**:详情页列出每次 run 的**状态、触发方式、时间、产出会话链接**;点链接可查看该次运行的完整对话。
- **取消**:对在飞的 run 可「取消」。
- **失败提示**:未读失败会在侧边栏显示红点;进入页面即清零。

---

## 二、原理

### 数据模型

- **`AutomationInfo`**:name、enabled、trigger_kind(schedule|manual)、cron、timezone、next_run_at、agent_type、root_folder_id、isolation、branch、config、last_run_*、unseen_failures 等。
- **`AutomationConfig`**(存 `config` JSON):`{ prompt_blocks, display_text, mode_id, config_values, label_snapshot }`。**触发时只读取前三项 + config**(prompt/agent 配置),label 快照仅用于展示。
- **`AutomationRunInfo`**:status(running/failed/…/skipped)、trigger、scheduled_for、started/ended_at、conversation_id、worktree_folder_id、stop_reason、error、summary。

后端:`commands/automation.rs`(CRUD 的 `*_core` + `run_now` / `cancel_run`)、`automation/engine.rs`(执行引擎)、`models/automation.rs`、`db/entities/automation{,_run}.rs`、`db/service/automation_service`。前端:`components/automations/*`、`lib/cron-humanize.ts`。

### 调度(定时)

- 引擎 `run_automation_engine` **每 30s** 一轮:`list_due` 找到期项 → `claim_due` 以 **CAS(compare-and-set)抢占**该时间槽(避免重复触发)→ 每个中标项 `tokio::spawn` 调 `run_automation(id, "schedule", slot)`。
- 手动触发走进程全局 `engine().run_automation(id, "manual", None)`。
- **下次运行时间**由权威函数 `automation_compute_next_run` 计算(cron + timezone),编辑器与列表都用它,保证一致。

### 拉起 agent(`AutomationEngine::launch`)

1. **per-automation fire-lock** 串行化;**重叠保护** `has_active_run`——同一自动化不并发,上一次还在跑就 `record_skipped_run`(记为 skipped)。
2. `start_run` 插入 run 行并**立即广播 `RunStarted`**(launch 可能耗数秒)。
3. **`resolve_cwd`**:`worktree_per_run` 建独立 worktree;`shared_in_root` 在根/指定分支上 checkout(**同一根目录串行**)。
4. 重算 env、`verify_agent_installed`(禁用或未安装 → 硬失败)。
5. `manager.spawn_agent(..., owner="automation", mode_id, config_values)` → `create_conversation_core` → 把 `conn_id → (run_id, automation_id)` 存进内存索引(供结果关联)→ `send_prompt_linked_with_message_id` 发出任务 prompt。
6. 全程多道 **cancel gate**;任一步失败即 `settle_run(Failed)` + 广播 `RunSettled(failed)`——**绝不静默挂起**。

### 回收结果(settle)—— 异步、fire-and-settle

- `run_automation` **不等**任务完成。事件总线的 **`TurnComplete`(按 connection_id)**经内存索引映射回 run,以 `stop_reason` 作为 settle 的权威依据。
- **兜底**:每 30s 的 `reconcile_once` 读产出会话的终态,补齐丢失的 `TurnComplete`;超过 **`MAX_RUN_MINUTES=180`** 的 run 被强制判失败。
- **崩溃恢复**:启动时 `boot_reconcile_interrupted` 把所有残留 `running` 视为中断——这依赖 **data-dir 独占文件锁**保证「本进程是唯一引擎」才敢这么做(多进程共享同一 data-dir 不安全)。
- **历史清理**:运行历史每 6h prune,保留 30 天。

### 事件

`AUTOMATION_CHANGED_EVENT`,含 `Upsert / Deleted / RunStarted / RunSettled`。前端每次事件**幂等全量 refetch**。

---

## 三、编辑器 UI 说明(简化点)

为降低误操作,编辑器做了两处简化:

- **运行方式**改为**明确的二选一分段控件**(「在文件夹内运行」/「每次运行新建 worktree」),取代原先「勾选 checkbox 会隐式隐藏分支选择器」的反直觉交互。选「在文件夹内运行」才出现分支选择器。
- **cron** 默认只展示**预设按钮 + 可视化「计划生成器」+ 下次运行预览**;**原始 cron 表达式**收进「**高级**」折叠区,需要精调时再展开。

---

## 四、常见排障

- **一直不触发**:确认 `enabled`;确认触发方式是 schedule 且 cron/时区正确(看「下次运行」预览);确认引擎在跑(进程存活、data-dir 锁未被其它进程占用)。
- **被 skipped**:上一次运行还没结束(`has_active_run`);缩短任务或改用 worktree 隔离减少串行等待。
- **运行失败**:看运行历史里的 `error` / `stop_reason`;常见为 agent 未安装/被禁用(`verify_agent_installed` 硬失败)、分支 checkout 冲突、或超过 180 分钟被强制判失败。
- **跨时区不准**:时区取本机;若在与预期不同的机器/时区上跑,cron 会按该机器时区解释。
