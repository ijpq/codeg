# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> **同步提醒**：本文件与根目录 `AGENTS.md` 除首行标题外内容完全一致。修改其中一份时务必同步另一份，避免两套代理指令漂移。

## 项目概述

Codeg（Code Generation）是一个多智能体编码工作台，它将多个智能体（Claude Code、Codex CLI、OpenCode、Gemini CLI、OpenClaw、Cline 等）统一到一个工作区中，支持会话聚合和多智能体协作，支持桌面安装，服务器/Docker 部署。

## 技术栈

- **桌面运行时**: Tauri 2（Rust 后端 + webview 前端）
- **服务器运行时**: 独立 Rust 二进制（Axum HTTP + WebSocket）
- **前端**: Next.js 16（静态导出模式）+ React 19 + TypeScript（strict）
- **样式**: Tailwind CSS v4 + shadcn/ui（radix-maia 风格）
- **国际化**: next-intl
- **数据库**: SeaORM + SQLite
- **包管理器**: pnpm

## 代码检查与测试（任务完成后进行必要的检查）

### 前端

```bash
pnpm eslint .                          # lint
pnpm test                              # vitest 全跑（CI 用同一条命令）
pnpm vitest run src/lib/utils.test.ts  # 跑单个测试文件
pnpm vitest run -t "名称片段"          # 按用例名过滤（单条测试）
pnpm test:watch                        # 开发时增量重跑
pnpm test:coverage                     # 覆盖率报告（输出到 coverage/index.html）
pnpm build                             # 静态导出构建
```

### 后端 Rust（在 `src-tauri/` 目录下执行）

```bash
# 桌面模式（默认 feature）
cargo check
cargo test --features test-utils
cargo test --features test-utils <name>                    # 跑单个单元测试
cargo test --features test-utils --test parsers_snapshot   # 跑单个集成测试文件（解析器快照在此）
cargo clippy --all-targets --features test-utils -- -D warnings

# 服务器模式
cargo check --no-default-features --bin codeg-server
cargo test --no-default-features --bin codeg-server --lib
cargo clippy --no-default-features --bin codeg-server --lib -- -D warnings

# codeg-mcp 协作伴生进程（多智能体委托）
cargo check --no-default-features --bin codeg-mcp
cargo clippy --no-default-features --bin codeg-mcp -- -D warnings

# 解析器快照评审（输出变化时；.snap 位于 src-tauri/tests/snapshots/）
cargo insta review
INSTA_UPDATE=auto cargo test --features test-utils     # 自动写新 .snap
```

## Git 分支与 worktree 强制生命周期

本项目允许使用 Git worktree 隔离并行开发，但 worktree 只是可重建的临时检出目录，
`origin` 上的分支、已合并的默认分支或已发布的远端 tag 才是持久化来源。不得把已结束的
worktree 及其构建缓存长期留在本机。

### 创建与使用

- 只有确实需要并行开发时才创建 worktree；串行工作继续使用现有工作区。
- 创建前先执行 `git fetch origin`。继续已有工作时，必须从对应的 `origin/<branch>`
  重建 worktree；新任务从明确的远端基线创建独立分支。
- worktree 应集中放在统一的 worktree 根目录（例如 `../codeg-worktrees/` 或项目配置的
  `worktree_root`），不得在 home 下随意散落新的 `codeg-*` 目录。
- 一个 worktree 只服务一个分支和一项并行任务，不得让多个代理共享同一检出目录。

### 结束与清理（严格按顺序执行）

1. 检查 worktree 的 tracked、staged、unstaged、untracked 和 ignored/build 产物，确认需要
   保留的修改均已提交；存在未处理文件时禁止删除目录。
2. 对仍需继续开发或尚未合并的分支，必须推送同名分支到 `origin` 并建立 upstream；远端
   tag 不能替代可继续开发的远端分支。
3. 对已经合并或发布的分支，可以不保留同名远端分支，但必须在 `git fetch origin` 后验证
   worktree 的 tip 已被 `origin` 默认分支包含，或验证对应 tag 确实存在于 `origin` 且指向
   正确提交。仅存在本地 tag、过期的 remote-tracking ref 或同名 GitHub Release 都不算验证。
4. 记录并核对本地 tip 与远端分支/tag 的 commit；push、fetch 或 commit 一致性检查失败时，
   必须停止清理并报告阻塞，不得假定内容已经备份。
5. 远端持久化验证通过后，使用 `git worktree remove <path>` 删除 worktree；不得直接用
   `rm -rf` 绕过 Git 的 worktree 安全检查。若 Git 因工作区不干净而拒绝删除，先处理原因，
   不得直接 `--force`。
6. 删除完成后执行 `git worktree prune` 清理失效登记，并确认对应目录以及其中的
   `target`、`.next`、`out`、`coverage` 等独立构建产物已经消失。
7. 本地分支 ref 可在远端验证后按需删除；以后需要继续开发时，重新 fetch 远端分支并创建
   新 worktree，不得依赖保留的历史工作目录。

### 完成判定

- 本地 tip 未被任何有效的 `origin` 分支或远端 tag 包含时，禁止删除其 worktree 或本地
  分支；临时 `backup/*` 也必须选择推送到远端归档分支或由用户明确授权放弃。
- 任务交付前必须审计 `git worktree list`。已完成任务的 worktree、失效登记和独立构建缓存
  仍然存在时，不得报告清理完成。
- 除当前主工作区和仍在进行的并行任务外，本地不应长期保留其他 Codeg worktree。

## 发版约定

当用户只说“发版”“release”或“完成 release 发版”时，直接执行
[`docs/release-workflow.md`](docs/release-workflow.md)，无需再次询问流程。除非用户指定版本或
发布通道，否则默认以 `upstream` 最新正式 Release 为基线，并发布同一基线下尚未使用的
下一个 `v<upstream-version>-fixN` 版本。默认资产仅包含 Windows x64 Setup、Windows x64
Server ZIP 及其校验文件。

固定顺序为：解析并拉取 `upstream` 最新已发布稳定版对应的 commit → 将本地待发版分支
rebase 到该 commit → 完成检查和版本提交 → 先推送分支到 `origin` 默认分支并确认该 commit
的普通 CI 通过 → 再推送版本 tag 触发 `origin` 的 Release workflow → 等待 Release CI 全部
通过并确认 GitHub Release 已正式发布。

不得仅因同名 Release 已存在就报告发版完成。最终必须核对本地 release commit、`origin`
默认分支包含的 commit、远端 tag 指向的 commit、Release workflow 的 `headSha` 四者一致。
若 CI 失败后又产生修复 commit，旧 tag/旧 Release 不代表新 commit 已发版，必须重新走版本
和发布校验流程。

## 架构

### 双模式运行

项目通过 Cargo feature flags 支持三种二进制：

- **`codeg`**（`tauri-runtime`，默认）：完整桌面应用，包含 Tauri 窗口管理、系统通知、自动更新等
- **`codeg-server`**（无 feature，`--no-default-features`）：独立服务器模式，仅编译 Axum HTTP API + WebSocket
- **`codeg-mcp`**（无 feature）：per-launch stdio MCP 伴生进程，被注入到代理 CLI 的 MCP 配置中，向 LLM 暴露**异步**子智能体委托工具。

### 共享核心

- **`app_state.rs`** — `AppState` 共享状态结构，两种模式通过 `EventEmitter` 枚举区分事件发射方式
- **`web/event_bridge.rs`** — `EventEmitter::Tauri(AppHandle)` 或 `EventEmitter::WebOnly(Arc<WebEventBroadcaster>)`
- **`web/router.rs`** — Axum 路由，接受 `Arc<AppState>`
- **`web/handlers/`** — HTTP API 端点，全部使用 `Extension<Arc<AppState>>`

### Rust 后端（`src-tauri/src/`）

后端负责读取和解析本地文件系统上的代理会话文件：

- **`app_state.rs`** — 共享状态（db、连接管理器、终端管理器、事件广播器）
- **`models/`** — 共享数据结构
- **`parsers/`** — 每个智能体一个解析器
- **`commands/`** — 业务逻辑，`_core` 函数供两种模式共用，`#[tauri::command]` 函数仅桌面模式
- **`web/`** — Axum HTTP API + WebSocket + 静态文件服务 + 认证中间件
- **`acp/`** — Agent Client Protocol 连接管理
- **`db/`** — SeaORM + SQLite

### 前端（`src/`）

#### 核心库（`lib/`）

- **`transport/`** — Transport 抽象层（自动检测 Tauri/Web 环境切换 `invoke()`/`fetch()`）
- **`adapters/`** — AI 响应到组件渲染的适配器
- **`types.ts`** — Rust 模型的 TypeScript 镜像
- **`api.ts`** — 主 API 客户端
- **`tauri.ts`** — Tauri API 封装

#### 国际化（`i18n/`）

- 支持 10 种语言：英语、简体中文、繁体中文、日语、韩语、西班牙语、德语、法语、葡萄牙语、阿拉伯语
- 使用 next-intl 框架，消息文件存放在 `i18n/messages/`

### 数据流

桌面模式：前端 `invoke()` → Tauri 命令 → 业务逻辑 → 返回数据
服务器模式：前端 `fetch()` → Axum HTTP API → 同一业务逻辑 → 返回 JSON
实时通信：后端事件 → EventEmitter（Tauri 事件 / WebSocket 广播）→ 前端

### 条件编译约定

- `#[cfg(feature = "tauri-runtime")]` — 仅桌面模式编译（Tauri 窗口、通知、`tauri::State` 参数等）
- `#[cfg_attr(feature = "tauri-runtime", tauri::command)]` — 函数始终可用，仅在桌面模式标记为 Tauri 命令
- `_core` 后缀函数 — 接受普通引用参数（`&AppDatabase`、`&EventEmitter`），供 Web handlers 和 Tauri 命令共用

## 关键约束

- **仅支持静态导出**：`next.config.ts` 设置 `output: "export"`，不支持动态路由（`[param]`），必须使用查询参数替代
- **路径别名**：`@/*` 映射到 `./src/*`，导入写法为 `@/lib/utils`、`@/components/ui/button`
- **服务器部署**：通过环境变量配置（`CODEG_PORT`、`CODEG_HOST`、`CODEG_TOKEN`、`CODEG_DATA_DIR`、`CODEG_STATIC_DIR`）
- **Docker 支持**：多阶段构建（Node.js + Rust），支持 `docker-compose` 一键部署
- **构建前置步骤**：`pnpm install` 的 `postinstall` 会把 `monaco-editor` 复制到 `public/vs`（编辑器依赖）；桌面构建/开发前需 `pnpm tauri:prepare-sidecars` 准备各代理 CLI 的 sidecar 二进制——`pnpm tauri:before-dev` / `tauri:before-build` 已包含该步骤

## 代码风格

- Prettier：无分号、尾逗号（es5）、2 空格缩进、80 字符宽度
- ESLint：next/core-web-vitals + typescript + prettier
- TypeScript：strict 模式，启用 `noUnusedLocals` 和 `noUnusedParameters`
- Rust：2021 edition，使用 `thiserror` 定义错误类型
