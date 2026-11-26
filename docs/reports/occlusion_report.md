# 面板本体被清空后"立方体浮到前面"——遮挡排查报告

> 状态：历史记录（2026-11-23 快照，随提交 `dacd61c` 入库）；其中“修 `rig_niri/src/main.rs` 的 `unhover_row` 补 rows 守卫”“下一步把 guard 应用到 rig_niri”“`render/probe/` 证据可复跑”已被当前实现取代，当前状态见 [docs/status/REMAINING.md](../status/REMAINING.md)。

> **已过时结论（2026-11-26 复核，原文一律保留）**
>
> 1. 定位锚点失效：`rig_niri/src/main.rs` 不在当前树（`git ls-tree HEAD rig_niri/src` 无 `main.rs`，该 crate 现在是 lib），`grep -rn "unhover"` 在 `rig_niri/src` 与 `feathers_in_3d/src` 均为 0 行。行底色现在是行模型自带的静态值：`Row::band` → `BackgroundColor`（`rig_niri/src/transcript.rs:399-400`），band 颜色取自主题的 17 个语义色（`rig_niri/src/rich.rs:15`），不存在“PointerOut 时把面板根/行背景写成 alpha=0”的代码路径。
> 2. 修复已结案，但记录它的提交不在当前 main 祖先链上：`3c02d69`（`fix(rig-feathers): PointerOut 冒泡清空面板背景（遮挡异常结案）`，author date 2026-11-23 06:25）的 `git merge-base --is-ancestor 3c02d69 HEAD` 退出码 1（历史经 jj 重写）；报告与探针随 `dacd61c` 入库，而 `dacd61c` 的 `rig-feathers/src` 已经是 lib（无 `main.rs`，`git grep PointerOut` 无结果），守卫的载体在那之前就没了。§4“在真正的 `rig_niri/src/main.rs` 应用 guard”因此已无对象。
> 3. 证据路径变了：`occlusion_probe/` 现在是 `experiments/occlusion_probe/`（HEAD 里 11 个文件，`git ls-files experiments/occlusion_probe`）；`render/probe/` 的截图与日志不在工作树，只能从历史提交取（`git show dacd61c:render/probe/…`），提交 `e5fb98c`（目录整理）把它从工作树删掉。
> 4. 行号锚点漂移：`feathers_in_3d/world3d.rs:168-173`/`:211` 现在对应 `world3d.rs:334`（读 `BackgroundColor`）与 `:726-756`（写 `mat.color`）；`sort.rs:50-67` 对应 `apply_panel_sort`（`sort.rs:51`）。函数名/类型名才是锚，行号以 2026-11-23 快照为准。
> 5. “`feathers_in_3d` 未改、库内无 bug”只是**这次遮挡排查**的结论，当时成立；库随后确有与本 bug 无关的渲染侧改动：透明排序覆写 `feathers_in_3d/src/sort.rs:1-37`（提交 `dad35f6`）。

**结论**：库里没问题。是 `rig_niri` 的 `unhover_row` 观察者缺少"这实体是不是一行"的判断，把**面板根实体**的背景色写成了 alpha=0；`feathers_in_3d` 忠实把这个颜色同步进 `UNodeMaterial3d`，SDF 的 body 因此全被 discard（边框仍画），背后的不透明立方体就从"洞"里露出来。看起来像立方体跑到前面，实际是面板本体消失了。与深度测试、透明排序、clip uniform、面板网格尺寸都无关。

复现：用 06:42 构建的 `rig_niri/target/debug/rig_niri` 可 100% 复现 → `render/probe/parent_repro/05_scrolled.png`（本体像素=清屏色，立方体像素=橙色）。把拷贝 `occlusion_probe/rig_dbg` 的 `PANEL_H` 改成 300 后，跑出的 `render/probe/repro300_bug/05_scrolled.png` 与报告的 300px 描述完全一致（面板深色底整块消失，橙色立方体出现在面板内、盖在内容上）；同一拷贝加一行 `rows` guard 后 `repro300_fix/05_scrolled.png` 恢复正常遮挡。

## 1. 复现步骤

### 1.1 最小 probe（新建 crate，未改 rig_niri）

`occlusion_probe/`：Camera3d + 不透明立方体 `(0.68,-0.18,-0.28)` + `WorldPanel/MirrorRoot` 面板（780×`PROBE_PANEL_H`，`Overflow::scroll_y`，默认字体、无 CJK）。

```bash
cd research/occlusion_probe
export CARGO_HOME=/var/home/lichi/桌面/bevy_3d_ai_research/.cargo-home CARGO_NET_OFFLINE=true
target/debug/occlusion_probe  # 或用 cargo run
# 例：
BEVY_ASSET_ROOT=$PWD PROBE_PANEL_H=300 PROBE_SCROLL=40 \
  PROBE_SHOT_DIR=$PWD/../render/probe/run300_40 timeout 120 target/debug/occlusion_probe
# 模拟"冒泡的 PointerOut 把面板根背景写成透明"：
BEVY_ASSET_ROOT=$PWD PROBE_PANEL_H=300 PROBE_SCROLL=40 PROBE_CLOBBER_BG=1 \
  PROBE_SHOT_DIR=$PWD/../render/probe/clobber300b timeout 120 target/debug/occlusion_probe
```

`a_before` 帧 70、`b_after` 帧 120；帧 30 的 `warmup` 用于规避 Bevy 首次 `Screenshot` 请求拿到空帧的问题（实测新进程第一张必空，与本次 bug 无关）。

### 1.2 现有二进制（就是 06:42 出问题那版）

```bash
cd research/rig_niri
BEVY_ASSET_ROOT=$PWD RF_SHOT_DIR=$PWD/../render/probe/parent_repro \
  timeout 120 ./target/debug/rig_niri
```

### 1.3 带日志的拷贝（只改拷贝）

`occlusion_probe/rig_dbg/` = rig_niri 源码 + `watch_panel_bg` / `PointerOver/Out` / `PointerHits` / pick index 日志；`Cargo.toml` 改名 `rig_dbg`、依赖路径改为 `../../feathers_in_3d`，target 用 btrfs reflink 复制所以不用重编依赖。**`rig_niri/` 本身没有任何改动。**

```bash
cd research/occlusion_probe/rig_dbg
CARGO_HOME=… cargo build
BEVY_ASSET_ROOT=/var/home/lichi/桌面/bevy_3d_ai_research/rig_niri \
  RF_SHOT_DIR=$PWD/../../render/probe/dbg_tour timeout 120 ./target/debug/rig_dbg
# 300px A/B（拷贝里 PANEL_H=300；guard 用环境变量开关，便于同一二进制对照）
unset RIG_UNHOVER_GUARD   # 复现
BEVY_ASSET_ROOT=… RF_SHOT_DIR=$PWD/../../render/probe/repro300_bug ./target/debug/rig_dbg
RIG_UNHOVER_GUARD=1       # 修复
BEVY_ASSET_ROOT=… RF_SHOT_DIR=$PWD/../../render/probe/repro300_fix ./target/debug/rig_dbg
```

## 2. 证据

### 2.1 像素（`(700,450)` 面板本体处；`(1020,505)` 立方体处）

| 截图 | 面板本体 | 立方体 | 说明 |
|---|---|---|---|
| `render/probe/parent_repro/01_overview.png` | (13,16,23) | (13,16,23) | 正常，立方体被遮住 |
| `render/probe/parent_repro/04_text_zoom.png` | **(43,44,47)=清屏色** | — | 本体已消失 |
| `render/probe/parent_repro/05_scrolled.png` | **(43,44,47)** | **(144,80,40)** | 立方体露出来 |
| `render/probe/parent_repro/06_selected.png` | **(43,44,47)** | **(144,80,40)** | 不会自愈 |
| `render/probe/dbg_tour/01_overview.png` | (13,16,23) | (13,16,23) | 正常 |
| `render/probe/dbg_tour/03a_mild_oblique.png` | (43,44,47) | — | 带日志版本坏得早一帧 |
| `render/probe/fix_tour/05_scrolled.png` | (22,30,46)=band | (13,16,23)=面板背景 | 加修复后立方体仍被遮住 |
| `render/probe/run300_0/b_after.png` | (13,16,23) | (13,16,23) | probe 300px 不滚动：**正常** |
| `render/probe/run300_40/b_after.png` | (13,16,23) | (13,16,23) | probe 300px+scroll=40：**正常** |
| `render/probe/clobber300b/a_before.png` | (13,16,23) | (13,16,23) | 模拟清空背景之前：正常 |
| `render/probe/clobber300b/b_after.png` | (43,44,47) | (144,80,40) | 仅把面板根背景 alpha 置 0：**现象复现** |
| `render/probe/repro300_bug/05_scrolled.png` | **(43,44,47)**（如 (700,560)） | **(102,61,51)** | **报告里的 300px 场景直接复现**（本体没了，立方体在面板内） |
| `render/probe/repro300_fix/05_scrolled.png` | (13,16,23) | (13,16,23) | 同代码 + `rows` guard：恢复正常遮挡 |

`render/probe/legacy/300_05_scrolled.png` 是 06:42 快照里的旧图（git blob），同一现象。

对比图：

- `render/probe/compare_artifact.png`：左上 380px 正常 / 右上 tour 05 坏掉（本体没了、立方体露出）/ 左下 probe 300px+scroll 正常 / 右下 probe 只把面板根背景 alpha 置 0（最小复现）。
- `render/probe/compare_fix.png`：左 坏掉 / 右 加 `rows` guard 后仍正常遮挡（380px）。
- `render/probe/compare_300.png`：左 `PANEL_H=300` 复现 / 右 同代码 + `rows` guard。

### 2.2 关键日志（`render/probe/dbg_tour/log.txt`）

```
56  PointerOver entity=505v0 is_row=true          # 某一行被 hover
57  PointerOut  entity=505v0                       # 它离开 hover（同一帧）
58  PointerOut  entity=457v0                       # 冒泡到祖先：面板根
65  BackgroundColor entity=457v0 is_panel_root=true ->
      Srgba { red:0.1, green:0.14, blue:0.22, alpha:0.0 }
```

- `bevy_picking` 0.20 的 `PointerOut` 定义就是会冒泡：`bevy_picking-0.20.1/src/events.rs:246-265`，`#[entity_event(propagate = PointerTraversal, auto_propagate)]`，文档写明 "Unlike `PointerLeave` … bubbles up to all of the target entity's ancestors"。
- 面板根**不是**被点中的：`render/probe/dbg_tour2/log.txt` 显示 `has_pickable=false`、`panel itself in PanelPickIndex=false`（14 个 rect 全是行）。所以事件只能来自子实体冒泡。
- 写入者是 `rig_niri/src/main.rs:321-333` 的 `unhover_row`：它只查 `selected`，**没有** `rows.contains(entity)`；对比 `hover_row`（`main.rs:305-319`）就有这个 guard。于是 `PointerOut` 冒泡到面板根时，面板根的 `BackgroundColor` 被写成 `srgba(0.10,0.14,0.22,0.0)`。（注：2026-11-26 已更新，`main.rs` 与 `unhover_row` 都不存在了，现状见文首“已过时结论”第 1 条。）
- `feathers_in_3d` 侧只是如实镜像：`world3d.rs:168-173` 从 `BackgroundColor` 取 `base_color`，`world3d.rs:211` 写 `mat.color`；clobber 后日志为 `node 330v0 … color=Vec4(0.0100,0.0174,0.0397, 0.0)`（`render/probe/clobber300/log.txt:89`）。
- shader 里 body 和 border 共用一次输出：`unode_3d.wesl:277-281` 的 `body_alpha`、`:366-369` 的 `shape_alpha`、`:415-419` 的 `discard`。`color.a=0` 时 body 贡献为 0，border 仍按 `border_color.a` 画出来——正好是截图里的"只有蓝色边框、没有深色底"。

### 2.3 已排除的假设（均有 probe 证据）

1. **材质 alpha mode / 队列**：立方体 `cube 329v0 alpha_mode=Opaque`；`Transparent3d` 只有 6 个 item，全部 `panel_item=true`、`cube_item=false`（`render/probe/run380_0/log2.txt:64-73`）。立方体不在透明队列，排序不可能把它放到面板后面/前面。
2. **`sort.rs` 覆写误伤非面板 item**：`sort.rs:50-67` 只改 `PanelSortTable` 里 `main_entity` 命中的 item；表由 `PanelSort` 组件填充（`sort.rs:38-48`），立方体没有 `PanelSort`。probe 中 32 个面板实体带 `PanelSort`、1 个 cube，透明 item 全命中面板。
3. **深度测试/深度写入**：面板透明、不写深度，但深度测试仍与不透明立方体比较；面板在 z=0、立方体在 z=-0.28，正常时面板覆盖立方体（`run300_40/b_after.png`）。probe 里把面板根背景 alpha 改 0 才会露立方体，说明区别只在"body 有没有画"。
4. **clip uniform**：probe dump 显示 380px 时 `clip_half=(0.702,0.342)`、300px 时 `(0.702,0.27)`，`clip_inv` 恒为单位矩阵；300px+scroll 下面板本体完整（`run300_40/b_after.png`）。滚动后裁剪也正确（内容被裁在面板框内，见 `clobber300b/a_before.png` 上方被裁掉的首行）。clip 不是元凶。
5. **面板 quad 尺寸/位置**：300px 时 `cached_size=(1.404,0.54)`、`local/world=(0,0,0)`；`ScrollPosition=40` 被布局 clamp 到 34px，只有子节点位移，面板本体不变形/不位移（`run300_40/log.txt:91`）。
6. **300px+滚动本身**：probe 的 300/0 与 300/40 都正常；只有模拟"面板根背景被写成透明"才复现（`clobber300b` 前后对比）。

## 3. 根因

```
行实体 PointerOut（鼠标/合成指针/相机移动都可能让它离开 hover）
  └─ bevy_picking 按 PointerTraversal 自动冒泡到所有祖先
       └─ rig_niri::unhover_row 没判 rows → 对面板根也执行
            └─ 面板根 BackgroundColor = srgba(…, alpha=0)
                 └─ world3d::sync_mirror 写 UNodeMaterial3d.color（alpha=0）
                      └─ SDF：body_alpha=0、border 照画 → 面板本体片元被 discard
                           └─ 后面的不透明立方体可见（看起来"浮到前面"）
```

**为什么是 300px+滚动时看到**：触发条件不是"面板变矮"，而是"有一行发生 PointerOut"。380 不滚动时，tour 的 01/05 截图前没有行级 PointerOut（两者同图）；300px+滚动改变内容相对指针的位置，加上 tour 的合成指针/相机移动，更容易让某一行离开 hover → 冒泡 → 背景清空，且 `hover_row` 对非行实体是早退的，之后的 `PointerOver` 不会把颜色恢复，所以立方体一直露着。父二进制的 380 版在相机移动时也会中招（04 起），说明和高度无关；300px 拷贝版即使内容不溢出（13 行，无需滚动）也照样复现，见 `repro300_bug/05_scrolled.png`。

## 4. 修复 / 建议

**改 `rig_niri`（本次不允许动，交给并行修改的同学）**，一行：

（注：2026-11-26 已更新——修复已合入历史、`main.rs` 已拆掉，下面这段补丁只作为当时的修改示意，见文首“已过时结论”第 1、2 条。）

```rust
fn unhover_row(
    out: On<bevy::picking::events::PointerOut>,
    rows: Query<&RowText>,                 // 新增
    mut backgrounds: Query<&mut BackgroundColor>,
    selected: Res<Selected>,
) {
    let entity = out.entity;
    // PointerOut 会冒泡到祖先；只有真正的行能丢 band 色
    if !rows.contains(entity) || selected.0.contains(&entity) {
        return;
    }
    if let Ok(mut bg) = backgrounds.get_mut(entity) {
        bg.0 = Color::srgba(0.10, 0.14, 0.22, 0.0);
    }
}
```

（另建议：unhover 时恢复该行的原始 band 色而不是 alpha=0；否则 hover 一次后 band 永久消失。`select_row` 的清理也有同样问题。补丁示意在 `occlusion_probe/unhover_row_fix.patch`。）

**已验证**：把该 guard 加到 `occlusion_probe/rig_dbg`（rig_niri 拷贝）后重跑同一 tour：
- `render/probe/fix_tour/01_overview.png`、`03a`、`04`、`05_scrolled`、`06_selected` 面板本体都在；
- `05_scrolled` 的立方体像素 `(1020,505)=(13,16,23)`=面板背景（仍被遮住）；
- `render/probe/fix_tour/log.txt:62-76` 仍有 `PointerOut entity=457v0 is_row=false`，但**不再有** `BackgroundColor 457v0 -> alpha 0` 的日志。

**`feathers_in_3d` 未改**：本轮排查未发现库内 bug；world3d/sort/text3d/pick 的行为均符合预期。如果在库里"防御性"忽略面板根的透明背景，会掩盖合法的透明面板，不应改。

### 下一步
- 在真正的 `rig_niri/src/main.rs` 应用上面的 guard（拷贝里已验证），用 300px+滚动+原来 15 行内容再跑一次 tour，确认 05/06 不再露立方体；顺便观察行 band 是否还在。
- 给行加"band 原始色"组件，unhover 时恢复，避免 hover 一次后 band 永久丢失这一类副作用。

（注：2026-11-26 已更新，以上两步都已完成/被取代：guard 随历史提交结案，`main.rs` 拆分后整条 hover-band 写法被 feathers 主题的静态 `Row::band` 替代；现状见文首“已过时结论”第 1、2、5 条与 `rig_niri/src/transcript.rs:399-400`。）

## 5. 产物

- `render/probe/compare_artifact.png`、`compare_fix.png`、`compare_300.png`：对比图。
- `occlusion_probe/`：独立 probe（env：`PROBE_PANEL_H`、`PROBE_SCROLL`、`PROBE_CLOBBER_BG`、`PROBE_FRONT_CUBE`、`PROBE_TOUR_CAM`、`PROBE_SHOT_DIR`）。
- `occlusion_probe/rig_dbg/`：rig_niri 的带日志拷贝（原目录未改动），用于定位与验证修复。
- `render/probe/`：`parent_repro/`（父二进制原样复现）、`dbg_tour/`、`dbg_tour2/`、`dbg_tour3/`（定位日志）、`fix_tour/`（修复验证）、`run300_0/`、`run300_40/`、`run380*/`（probe 对照组）、`clobber300b/`（最小复现）、`repro300_bug/`、`repro300_fix/`（300px 原始现象 A/B）、`legacy/`（06:42 快照旧图）、`run380_0/log2.txt`（渲染队列 dump）。
