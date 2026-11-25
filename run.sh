#!/usr/bin/env bash
# 一条命令把这个 3D app 跑起来。
#
#   ./run.sh              # 单会话，演示巡游
#   ./run.sh ws3          # 三条会话条带，竖直排列（F1..F8 聚焦、F9 取消、F10 自动整理）
#   ./run.sh niri         # niri 平面布局
#   ./run.sh shots        # 跑完把截图写到 render/rf_niri_app/
#
# 为什么需要这个脚本：程序要三样东西，缺一样就会出现"窗口打开了但什么都没看见"——
#   1. CARGO_HOME  —— 离线依赖目录（本机没有全局 cargo 缓存）
#   2. BEVY_ASSET_ROOT —— 字体在 rig_harness_in_3d/assets，cwd 不对就加载不到（字会消失）
#   3. RIG_HOME    —— harness 要在这里建会话目录，默认 ~/.rig 在只读环境下会失败
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export CARGO_HOME="$ROOT/.cargo-home"
export BEVY_ASSET_ROOT="$ROOT/rig_harness_in_3d"
export RIG_HOME="${RIG_HOME:-$ROOT/.rig-home}"
mkdir -p "$RIG_HOME"

# workspace 根统一构建（三个 crate 一份 target / 一份锁文件）
cd "$ROOT"

case "${1:-}" in
  niri)  export RF_NIRI_LAYOUT=1 ;;
  ws3)   export RF_WORKSPACES=3 ;;
  ws5)   export RF_WORKSPACES=5 ;;
  shots) export RF_SHOT_DIR="$ROOT/render/rf_niri_app"; mkdir -p "$RF_SHOT_DIR" ;;
  "")    ;;
  *)     echo "用法: ./run.sh [niri|shots]" >&2; exit 2 ;;
esac

echo "窗口里应该看到：转写面板（左）+ 输入框/状态行（右）；多条条带时竖直排列。"
echo "  鼠标中键（或 Alt+左键）拖动条带；F10 自动整理；F1..F8 切聚焦；F9 取消聚焦。"
echo "  RIG_HOME=$RIG_HOME"
echo "  BEVY_ASSET_ROOT=$BEVY_ASSET_ROOT"
exec cargo run --offline -p rig_harness_in_3d
