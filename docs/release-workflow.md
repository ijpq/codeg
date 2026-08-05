# CodeG Fork 发版流程

本文定义 `origin`（当前 fork）的标准发版流程。用户说“发版”“release”或“完成 release
发版”时，代理应直接执行本流程；只有遇到无法安全判断待发布改动、rebase 冲突、权限失败或
签名/CI 等实际阻塞时才请求用户处理。

## 默认约定

- `upstream` 是 `https://github.com/xintaofei/codeg.git`，`origin` 是待发布的 fork。
- 基线取 `upstream` 最新**已发布、非 draft、非 prerelease**的 GitHub Release 所指向的
  commit，不使用可能领先于正式版的 `upstream/main`。用户明确要求 RC 或其他通道时按其
  指定执行。
- 待发版分支是用户当前工作的本地分支，通常为 `main`。
- 用户未指定版本时，以 `origin` 最新稳定 Release 为基础递增 patch 版本。
- Release workflow 由 `v*.*.*` tag push 触发；tag 必须与
  `src-tauri/tauri.conf.json` 中的版本一致，而且 tag commit 必须属于 `origin` 默认分支。

## 1. 发布前确认

1. 记录当前分支、`origin`/`upstream` 地址和工作区状态。
2. 确认本次应该发布的修改均已提交，不能把无关的用户改动、临时文件或构建产物一起提交。
3. 确认 GitHub CLI 当前账号有向 `origin` 推送和创建 Release 的权限。
4. 记录 rebase 前的分支和 commit，发生冲突时不得丢弃用户改动。

参考检查：

```bash
git branch --show-current
git status --short
git remote -v
gh auth status
gh repo view --json nameWithOwner,defaultBranchRef
```

工作区存在未提交改动时，应先将本次待发布改动整理成 commit；无法区分无关改动时停止并说明
阻塞，不得擅自清理、覆盖或把所有文件一并提交。

## 2. 拉取 upstream 最新 Release commit

先查询 `upstream` 最新稳定 Release tag，再把该 tag 拉到 upstream 专用引用，避免它与
`origin` 自定义版本的同名 tag 互相覆盖：

```bash
UPSTREAM_REPO=xintaofei/codeg
UPSTREAM_TAG=$(gh release list --repo "$UPSTREAM_REPO" --exclude-drafts --exclude-pre-releases --limit 1 --json tagName --jq '.[0].tagName')
git fetch upstream --prune
git fetch upstream "+refs/tags/${UPSTREAM_TAG}:refs/remotes/upstream/releases/${UPSTREAM_TAG}"
UPSTREAM_SHA=$(git rev-parse "refs/remotes/upstream/releases/${UPSTREAM_TAG}^{commit}")
git show --no-patch --oneline "$UPSTREAM_SHA"
```

`UPSTREAM_TAG` 或 `UPSTREAM_SHA` 为空时不得继续。不要用本地旧 tag 猜测 upstream 最新
Release，也不要默认改为 rebase `upstream/main`。

## 3. Rebase 待发版分支

切回待发版分支，将其 rebase 到上一步解析出的精确 Release commit：

```bash
git switch <release-branch>
git rebase "$UPSTREAM_SHA"
git merge-base --is-ancestor "$UPSTREAM_SHA" HEAD
```

出现冲突时逐项审查并解决，之后继续 rebase；不得通过删除本地功能或跳过 commit 来换取
表面成功。rebase 完成后重新检查 diff，并运行与改动风险相匹配的前后端测试。

## 4. 版本和 release commit

默认选择 `origin` 最新稳定版本的下一个 patch 版本。同步更新以下版本来源：

- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/Cargo.lock` 中 CodeG 自身 package 的版本
- `src-tauri/tauri.conf.json`

版本一致且必要检查通过后创建 release commit，格式使用：

```text
release: <version> <简短发布主题>
```

随后记录唯一的预期发布 commit：

```bash
EXPECTED_SHA=$(git rev-parse HEAD)
RELEASE_TAG="v<version>"
```

从此处开始，任何代码或配置修复都会改变 `EXPECTED_SHA`，必须重新检查、提交并重新确认
版本/tag；不能继续拿旧 Release 当作当前代码已经发布。

## 5. 先推分支并等待普通 CI

先拉取 `origin` 最新状态，再将 rebased 分支推到 `origin` 默认分支。rebase 导致远端历史
需要重写时只允许使用 `--force-with-lease`，不得使用无保护的 `--force`：

```bash
git fetch origin --prune
git push --force-with-lease origin <release-branch>:<origin-default-branch>
```

等待该 `EXPECTED_SHA` 对应的 `.github/workflows/test.yml` 成功。可以通过以下方式定位并等待：

```bash
gh run list --workflow test.yml --commit "$EXPECTED_SHA" --limit 1 --json databaseId,headSha,status,conclusion
gh run watch <run-id> --exit-status
```

普通 CI 未通过时不要创建 release tag。修复并产生新 commit 后，更新 `EXPECTED_SHA`，重新
推送并等待新 commit 的 CI；旧 commit 的成功记录不能代替新 commit。

## 6. 推 tag，完成 origin Release

确认远端默认分支包含 `EXPECTED_SHA`，且目标 tag 尚未被其他发布占用：

```bash
git fetch origin <origin-default-branch>
git merge-base --is-ancestor "$EXPECTED_SHA" "origin/<origin-default-branch>"
git ls-remote --exit-code origin "refs/tags/${RELEASE_TAG}"
```

最后一条命令返回未找到才可创建新 tag；如果 tag 已存在，不得静默移动它，应检查是否已指向
同一 commit。新版本使用 annotated tag：

```bash
git tag -a "$RELEASE_TAG" "$EXPECTED_SHA" -m "codeg ${RELEASE_TAG#v}"
git push origin "refs/tags/$RELEASE_TAG"
```

tag push 会触发 `.github/workflows/release.yml`。必须定位 `headSha == EXPECTED_SHA` 的运行，
等待全部 job 成功：

```bash
gh run list --workflow release.yml --commit "$EXPECTED_SHA" --limit 1 --json databaseId,headSha,status,conclusion,url
gh run watch <run-id> --exit-status
```

## 7. 完成判定

只有以下条件全部满足，才能向用户报告“发版完成”：

1. `upstream` 最新 Release commit 是本次 rebase 基线。
2. `origin` 默认分支包含 `EXPECTED_SHA`。
3. `origin` 远端 `RELEASE_TAG` peel 后精确指向 `EXPECTED_SHA`。
4. Release workflow 的 `headSha` 精确等于 `EXPECTED_SHA` 且 conclusion 为 `success`。
5. `gh release view "$RELEASE_TAG"` 显示 Release 存在且 `isDraft=false`。
6. Release 页面和必要 assets 已生成；不能只看到 draft 或旧的同名 Release 就结束。

建议最终核对：

```bash
git fetch origin <origin-default-branch>
git merge-base --is-ancestor "$EXPECTED_SHA" "origin/<origin-default-branch>"
git ls-remote origin "refs/tags/${RELEASE_TAG}" "refs/tags/${RELEASE_TAG}^{}"
gh run list --workflow release.yml --commit "$EXPECTED_SHA" --limit 1 --json headSha,status,conclusion,url
gh release view "$RELEASE_TAG" --json tagName,isDraft,isPrerelease,publishedAt,url
```

最终报告至少写明：upstream 基线 tag/commit、本地 release commit、origin tag、普通 CI 结果、
Release CI 结果和 GitHub Release 地址。

## CI 失败后的规则

- 只是 GitHub runner、下载源等瞬时基础设施问题且代码未变，可以重跑同一 SHA 的 workflow。
- 任何修复导致 commit 变化，都必须更新 `EXPECTED_SHA`，重新推分支、等待普通 CI，再触发与
  新 commit 对应的发布。
- 已发布 tag 原则上不可变。若已发布版本需要代码修复，默认递增 patch 版本并创建新 tag；
  不得把旧 release 的存在当成修复 commit 已经发布。
- `workflow_dispatch` 在当前仓库只用于修复既有 tag 的 server assets，不等价于一次新的完整
  Release，也不能用它证明新 commit 已发布。
