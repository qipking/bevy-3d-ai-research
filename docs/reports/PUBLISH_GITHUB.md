# 上传到 GitHub（2026-11-25）

> 状态：历史记录（2026-11-25 上传/推送手册快照）；其中“需要你提供一种认证方式才能上传”“历史从 `feat: 3D 版 rig harness 的第一个可用骨架` 开始”已被当前状态取代，当前状态见 [docs/status/REMAINING.md](../status/REMAINING.md)；推送现状见下方“已过时结论”。

## 推送现状与已过时结论（2026-11-26 复核，原文一律保留）

1. **上传已完成，可核对**：`origin` = `https://github.com/qipking/bevy-3d-ai-research.git`（`.git/config`），`HEAD` = `main` = `origin/main` = `b22d674`；GitHub API 2026-11-26 复核：`private=false`、`default_branch=main`、`pushed_at=2026-11-25T23:36:15Z`。原文“需要你提供一种认证方式”只对**将来推新提交**成立，不再是“仓库还没上传”。
2. **本机仍然“无凭证”**：`which gh` 为空、`~/.ssh` 下无 `*.pub`、`git config --get credential.helper` 为空、remote URL 里没有 token。推新提交仍需 token / SSH / helper 三选一。
3. **提交链起点变了**：当前 main 的第一个提交是 `ee5519b baseline: 路线B验证 + rig-tui 源码分析 + rig-feathers 适配层初版`（`git log --reverse --oneline | head -1`）；`dacd61c feat: 3D 版 rig harness 的第一个可用骨架` 在链中间，不是起点——历史经 jj 重写。
4. **jj + colocated git 仍是现状**：`jj 0.41.0`，bookmark `main` 指向 `b22d6746`，`jj status` = “The working copy has no changes”，`HEAD` == `main` == `b22d674`，工作副本 `@` 是空提交 `bcbf9fa1`。
5. **内容表基本仍准确，数字更新**：`vendor/bevy_luma` 14M；`render/rf_niri_app` 9.0M（原文 12M）；`reference/rig-harness-src` 632K；`experiments` 684K；`tools/pylib` 9.6M 且仍 gitignore；`reference/rig-official` 只 tracked `README.md` + `capture.py`；`reference/niri`、`reference/rig-tui-inspect` 仍 gitignore；`.cargo/config.toml` 存在，`cargo niri` alias 在 `:3`。bundle 仍在：`/var/home/lichi/桌面/bevy_3d_ai_research.bundle`（92M，2026-11-25 08:30）。
6. **凭据注意（补充）**：方式 B 会把 PAT 写进 `remote.origin.url`，也就落进 `.git/config`；除原文“立刻 set-url 还原”外，token **不要写进 documents/ 与命令历史**（不要在 `git remote add` 里带 PAT、不要把 PAT 贴进日志），优先 credential helper 或一次性 `GIT_ASKPASS`。推送遇 GitHub 网络波动（HTTP/2 reset、`RPC failed`）用 `git -c http.version=HTTP/1.1 git push` 重试。
7. **密钥扫描复核**：2026-11-26 对 HEAD 扫 `ghp_` / `sk-` / `github_pat_` / 私钥头（`git grep`），无匹配；`.git/config` 无 token。原文“已扫描无密钥泄漏”仍成立（只限仓库内容）。

> **已上传：https://github.com/qipking/bevy-3d-ai-research （public，默认分支 main）**
>
> 这台机器上**没有任何 GitHub 凭证**：没有 `gh` CLI，没有 SSH key，`~/.gitconfig` 为空，
> 没有 credential helper。所以推送需要你提供一种认证方式（见下面三选一）。（注：2026-11-26 复核，凭证状况仍如此，但仓库已上传，见上方第 1、2 条。）

仓库本身已经准备好了：

- 分支：`main`（指向最新提交，jj 的工作副本已提交，工作区干净）；
- 历史：从 `feat: 3D 版 rig harness 的第一个可用骨架` 开始的完整提交链；（注：2026-11-26 已更新，当前起点是 `ee5519b baseline`，见上方第 3 条。）
- `.gitignore` 已覆盖：`target/`、`.cargo-home/`、`.rig-home/`、`reference/niri/`、
  `reference/rig-official/`（源码副本）、`reference/rig-tui-inspect/`、调试截图；
- 已扫描无密钥泄漏（`sk-*`、`ghp_*`、私钥）。（注：2026-11-26 复核无匹配，见上方第 7 条。）

## 方式 A：你自己推（推荐，凭证不经过任何脚本）

仓库根目录已经生成好一份可直接克隆/推送的 bundle：

```sh
/var/home/lichi/桌面/bevy_3d_ai_research.bundle    # 92M，含完整历史
```

1. 在 GitHub 上新建空仓库（不要初始化 README）；
2. 在任意有凭证的机器上：

```sh
git clone /path/to/bevy_3d_ai_research.bundle bevy_3d_ai_research
cd bevy_3d_ai_research
git remote add origin git@github.com:<你>/<仓库名>.git
git push -u origin main
```

或者直接用本仓库（已经配好 `main` 分支）：

```sh
cd /var/home/lichi/桌面/bevy_3d_ai_research
git remote add origin git@github.com:<你>/<仓库名>.git
git push -u origin main
```

## 方式 B：用 Personal Access Token（一次性）

```sh
cd /var/home/lichi/桌面/bevy_3d_ai_research
git remote add origin https://<github用户名>:<PAT>@github.com/<你>/<仓库名>.git
git push -u origin main
git remote set-url origin https://github.com/<你>/<仓库名>.git   # 立刻把 token 从配置里去掉
```

PAT 只需要 `repo` 权限（`public_repo` 就够公开仓库）。（注：2026-11-26 补充——token 不要写进 documents/ 与命令历史；推送网络波动用 `git -c http.version=HTTP/1.1 git push`，见上方第 6 条。）

## 方式 C：给这台机器配 SSH key

```sh
ssh-keygen -t ed25519 -C "lichi@localhost" -f ~/.ssh/id_ed25519 -N ''
cat ~/.ssh/id_ed25519.pub      # 贴到 GitHub → Settings → SSH and GPG keys
ssh -T git@github.com          # 看到 "Hi <user>!" 就通了
```

之后按方式 A 的 `git push` 即可。

## 仓库要带哪些东西（现状）

| 目录 | 进不进仓库 | 说明 |
|---|---|---|
| `rig_harness_in_3d/`、`rig_niri/`、`feathers_in_3d/` | ✅ | 三个 crate（workspace members） |
| `Cargo.toml`、`Cargo.lock`、`.cargo/config.toml` | ✅ | workspace 与离线运行配置 |
| `vendor/bevy_luma/` | ✅ | vendored 依赖（14M） |
| `docs/` | ✅ | 设计/状态/报告/归档 |
| `reference/rig-harness-src/` | ✅ | 上游 harness 源码（只读参考） |
| `reference/niri/` | ❌ | GPL-3.0，只作语义参考，已 gitignore |
| `reference/rig-official/` 源码 | ❌ | 可从 crates.io 一键重建（README 里有命令），只提交 `README.md` + `capture.py` |
| `reference/rig-tui-inspect/` | ❌ | 解包的 crate 源码，可重建 |
| `experiments/` | ✅ | 两个一次性探针（各自独立构建） |
| `render/rf_niri_app/` | ✅ | 最近一次的界面证据（12M） |
| `tools/pylib/` | ❌ | Python 依赖，可重装 |
| `target/`、`.cargo-home/`、`.rig-home/` | ❌ | 构建产物与运行期会话 |

## 仓库描述建议

```
用 Bevy 3D 重写 rig-harness 界面的实验：rig_niri（视图层，niri 分裂树平铺）
+ feathers_in_3d（渲染层）+ rig_harness_in_3d（应用层）
```
