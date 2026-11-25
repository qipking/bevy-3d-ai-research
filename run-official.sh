#!/usr/bin/env bash
# 跑**官方** rig（终端 TUI），用来对照我们 3D 版的行为。
#
#   ./run-official.sh            # 直接进界面
#   ./run-official.sh --build    # 二进制不在时先编译官方 rig
#
# 界面里可以先试：
#   /            补全命令（和我们的补全对照）
#   /help        命令列表
#   /model       选模型（要真用需要一个 key，不选也能看界面）
#   Ctrl+D       退出
#
# 注意：这是终端 UI，必须在**真终端**里跑（在网页/无 TTY 的环境里跑不起来）。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export CARGO_HOME="$ROOT/.cargo-home"          # 离线依赖目录（本机没有全局 cargo 缓存）
export RIG_HOME="${RIG_HOME:-$ROOT/.rig-home}" # 官方 agent 的项目/会话目录，已经建过一次
mkdir -p "$RIG_HOME"

SRC="$ROOT/reference/rig-official"
BIN="$SRC/target/debug/rig"

if [[ ! -x "$BIN" ]]; then
  echo "还没编译官方 rig，先编（约 1 分钟）："
  echo "  cd $SRC && CARGO_HOME=$CARGO_HOME cargo build --offline --bin rig"
  exit 1
fi

exec "$BIN" "$@"
