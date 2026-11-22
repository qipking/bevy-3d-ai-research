#!/bin/sh
# 生成 CJK 字体（Bevy 只认 .ttf/.otf，系统里是 .ttc 集合）。
# 产物不进版本库（十几 MB），克隆后跑一次即可。
set -e
cd "$(dirname "$0")/.."
SRC=${1:-/usr/share/fonts/google-noto-sans-cjk-vf-fonts/NotoSansCJK-VF.ttc}
[ -f "$SRC" ] || { echo "missing $SRC (install google-noto-sans-cjk-vf-fonts)"; exit 1; }
mkdir -p rig-feathers/assets/fonts
python3 tools/ttc_extract.py "$SRC" rig-feathers/assets/fonts/NotoSansCJKsc-Regular.otf --pick-sc
