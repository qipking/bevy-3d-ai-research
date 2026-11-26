# 官方 `rig` 真实界面（2026-11-25 抓取）

> 状态：历史记录（2026-11-25 抓取官方 rig v0.47.0 终端界面）；其中“提示行与我方逐字相同”“状态行 `no model` 占位与我们的 status.rs 一致”已被当前实现改写，当前状态见 [docs/status/REMAINING.md](../status/REMAINING.md)。

> **已过时结论（2026-11-26 复核，原文一律保留）**
>
> 1. 提示行不再逐字相同：当前 `Ctrl+V` 一条按本 crate 的实际行为写 `paste`（`rig_niri/src/hints.rs:42-43` 的注释、`:51`/`:59`/`:373` 的文案），官方是 `image`；其余键位仍逐字对齐。
> 2. 抓取里的 `no model: /model picks one` 是**官方空会话的占位**，不是当前 3D 版的现状：我们的状态行复用上游条目，没有 agent 时补 `no agent`（`rig_niri/src/status.rs:13-14`、`:94`）；真实模型接入未做（`docs/status/HANDOFF.md:159`），也不在本仓库范围（`docs/status/REMAINING.md:12-13`）。
> 3. 复核仍成立：rig 版本线与抓取版本一致（`rig_niri/Cargo.toml:30` 的 `rig-tui = "=0.47.0"`）；布局仍有意不照搬官方位置槽，用 niri 分裂树（`rig_niri/src/layout.rs`、`niri.rs`）；补全落点仍是输入框上方 tile（`complete.rs:1093` 的 `sync_completion_tile`，树里 `layout.rs:1158` 的注释指向它）；抓取工具仍在仓库，但路径已从原文的 `experiments/rig-official/capture.py` 移到 `reference/rig-official/capture.py`（`git ls-files reference/rig-official`，目录整理 `e5fb98c`）。
> 4. 标题里的 2026-11-25 只是抓取日，不代表当前状态。

> 目的：移植前先看官方长什么样。以前只读源码猜，这是第一次**真跑起来**。
> 抓法：`experiments/rig-official/capture.py`（伪终端 + 去 ANSI），命令、原始字节都在那儿。（注：2026-11-26 已更新，路径现为 `reference/rig-official/capture.py`。）

## 启动（空会话）

```
        █▙            rig v0.47.0
        ▟█
        ██            no model: /model picks one
        █▛
        ▜█
  ~/桌面/research/experiments/rig-official
  Enter sends · Shift+Enter or Ctrl+J new line · Ctrl+V image · / commands · @ files · Ctrl+D quits
  ›
  no model: /model picks one          reasoning default     idle
```

窗口标题：`rig · idle`（OSC 2）。

## 输入 `/he`（补全弹出）

```
  /help
    List the commands
  Tab or Enter completes · Esc closes
  › /he
```

## 由此确认的事实

1. **官方布局就是"绝对位置槽"**：`rig-tui/src/screen.rs:28` 用的是
   `TuiPanel::new(Placement::Bottom(Constraint::Length(rows)))` —— 底部锚定长度 + 剩余区。
   它**没有**平铺/分列的概念。
2. **补全不是浮层**：候选 + 缩进说明 + 一行 `Tab or Enter completes · Esc closes` 提示，
   就贴在输入框**上方**；不是居中浮窗。
3. **状态行**：左 `no model: /model picks one`，右 `reasoning default` / `idle`；
   与我们的 `status.rs` 结构一致。（注：2026-11-26 已更新，`no model` 是官方占位；我们没有 agent 时补 `no agent`，见文首“已过时结论”第 2 条。）
4. **提示行文案**与我们移植的逐字相同（`Enter sends · Shift+Enter or Ctrl+J new line ·
   Ctrl+V image · / commands · @ files · Ctrl+D quits`），尺寸也够宽（120 列不折行）。（注：2026-11-26 已更新，`Ctrl+V` 一条现写作 `paste`，见文首“已过时结论”第 1 条与 `rig_niri/src/hints.rs:42-43`。）

## 结论（对移植的影响）

- **内容与交互**：照官方搬，已经基本一致（转写 / 状态 / 输入 / 提示 / 补全）。
- **布局**：官方是位置槽，**3D 版不照搬** —— 用户要求 `rig_niri` 用 niri 的
  **分裂树**（列 + 列内竖分 + 可再拆），而不是组合 pos。这是有意偏离，不是移植缺口。
- **补全落点**：定在输入框**上方的 tile**（niri 里就是输入列里多出来的一块），
  不是 `Placement::Over` 浮层。
