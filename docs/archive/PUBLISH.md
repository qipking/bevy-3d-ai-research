# 三个 crate 的发布准备

> **状态：已归档，仅历史参考；当前发布记录见 [reports/PUBLISH_GITHUB.md](../reports/PUBLISH_GITHUB.md)。**

## 仓库布局（建议：每个 crate 一个 GitHub 仓库）

```
feathers_in_3d/       -> github.com/<you>/feathers_in_3d      （3D 渲染底座，无业务）
rig_niri/         -> github.com/<you>/rig_niri        （rig-tui 的 3D 视图层，lib）
rig_harness_in_3d/    -> github.com/<you>/rig_harness_in_3d   （harness 层 fork 模板，lib + demo）
research/             -> 私有工作区（不发布：证据、spike、occlusion_probe、tools）
```

## 依赖关系（发布顺序）

```
feathers_in_3d  ←  rig_niri  ←  rig_harness_in_3d
```

`Cargo.toml` 里现在的 `path = "../feathers_in_3d"` 在**发布时**要改成：

```toml
feathers_in_3d = { version = "0.1", path = "../feathers_in_3d" }   # 发 crates.io 用
# 或者（不上 crates.io，只在 GitHub 上 fork 用）
feathers_in_3d = { git = "https://github.com/<you>/feathers_in_3d", tag = "v0.1.0" }
```

**fork 模板只需要最后一个仓库**（`rig_harness_in_3d`），前两个作为依赖拉取。

## 每个仓库要做的事

1. `.gitignore`：
   ```
   /target
   /assets/fonts/*.otf
   ```
2. `Cargo.toml` 补元数据：`description` / `license = "MIT"` / `repository` / `keywords` / `categories`；
3. `README.md`：本仓库已有（`feathers_in_3d/README.md`、`rig_niri/README.md`、`rig_harness_in_3d/README.md`）；
4. `LICENSE`：rig 上游是 MIT/Apache-2.0，建议与上游一致（双许可）；
5. `git init` + 首次提交 + `gh repo create`（或网页建仓后 `git push`）。

## 上游版本锁定（发布前确认）

| crate | 版本 |
|---|---|
| bevy / bevy_feathers / bevy_ui* | `=0.20.1` |
| rig-tui / rig-core / rig-ecs / rig-harness | `=0.47.0` |
| bevy_luma | 上游 git main（0.20）；发布时建议 fork 一份并打 tag |

## 注意

- `rig_niri` 的 `UPSTREAM.md` 是**给 fork 者的契约**：说明我们依赖 rig 的哪些公开 API；
- `feathers_in_3d` 不依赖 rig，可独立发布/复用；
- 三个 crate 都还没有 `LICENSE` 文件，发布前补。
