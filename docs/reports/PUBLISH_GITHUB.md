# 上传到 GitHub（2026-11-25）

> **已上传：https://github.com/qipking/bevy-3d-ai-research （public，默认分支 main）**
>
> 这台机器上**没有任何 GitHub 凭证**：没有 `gh` CLI，没有 SSH key，`~/.gitconfig` 为空，
> 没有 credential helper。所以推送需要你提供一种认证方式（见下面三选一）。

仓库本身已经准备好了：

- 分支：`main`（指向最新提交，jj 的工作副本已提交，工作区干净）；
- 历史：从 `feat: 3D 版 rig harness 的第一个可用骨架` 开始的完整提交链；
- `.gitignore` 已覆盖：`target/`、`.cargo-home/`、`.rig-home/`、`reference/niri/`、
  `reference/rig-official/`（源码副本）、`reference/rig-tui-inspect/`、调试截图；
- 已扫描无密钥泄漏（`sk-*`、`ghp_*`、私钥）。

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

PAT 只需要 `repo` 权限（`public_repo` 就够公开仓库）。

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
