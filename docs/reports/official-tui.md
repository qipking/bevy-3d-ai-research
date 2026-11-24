# 官方 `rig` 真实界面（2026-11-25 抓取）

> 目的：移植前先看官方长什么样。以前只读源码猜，这是第一次**真跑起来**。
> 抓法：`experiments/rig-official/capture.py`（伪终端 + 去 ANSI），命令、原始字节都在那儿。

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
   与我们的 `status.rs` 结构一致。
4. **提示行文案**与我们移植的逐字相同（`Enter sends · Shift+Enter or Ctrl+J new line ·
   Ctrl+V image · / commands · @ files · Ctrl+D quits`），尺寸也够宽（120 列不折行）。

## 结论（对移植的影响）

- **内容与交互**：照官方搬，已经基本一致（转写 / 状态 / 输入 / 提示 / 补全）。
- **布局**：官方是位置槽，**3D 版不照搬** —— 用户要求 `rig_niri` 用 niri 的
  **分裂树**（列 + 列内竖分 + 可再拆），而不是组合 pos。这是有意偏离，不是移植缺口。
- **补全落点**：定在输入框**上方的 tile**（niri 里就是输入列里多出来的一块），
  不是 `Placement::Over` 浮层。
