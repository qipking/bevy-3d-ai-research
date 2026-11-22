#!/bin/sh
# 生成 CJK 字体（Bevy 只认 .ttf/.otf，系统里是 .ttc 集合）。
# 产物不进版本库（十几 MB），克隆后跑一次即可。
set -e
cd "$(dirname "$0")/.."
SRC=${1:-/usr/share/fonts/google-noto-sans-cjk-vf-fonts/NotoSansCJK-VF.ttc}
[ -f "$SRC" ] || { echo "missing $SRC (install google-noto-sans-cjk-vf-fonts)"; exit 1; }
mkdir -p rig-feathers/assets/fonts
python3 tools/ttc_extract.py "$SRC" rig-feathers/assets/fonts/NotoSansCJKsc-Regular.otf --pick-sc

# 2) 子集化：30 MB -> ~3 MB（ASCII + CJK 标点 + GB2312 常用字）
#    需要 fonttools：pip3 install --target tools/pylib fonttools brotli
if PYTHONPATH="$PWD/tools/pylib" python3 -c "import fontTools" 2>/dev/null; then
  PYTHONPATH="$PWD/tools/pylib" python3 tools/make_cjk_charset.py common > /tmp/cjk_unicodes.txt
  PYTHONPATH="$PWD/tools/pylib" python3 -m fontTools.subset \
    rig-feathers/assets/fonts/NotoSansCJKsc-Regular.otf \
    --output-file=rig-feathers/assets/fonts/NotoSansCJKsc-Subset.otf \
    --unicodes="$(cat /tmp/cjk_unicodes.txt)" --layout-features='*' --name-IDs='*'
  echo "subset -> $(du -h rig-feathers/assets/fonts/NotoSansCJKsc-Subset.otf | cut -f1)"
else
  echo "fonttools missing: 用完整字体（30 MB）。装法：pip3 install --target tools/pylib fonttools brotli"
  cp rig-feathers/assets/fonts/NotoSansCJKsc-Regular.otf rig-feathers/assets/fonts/NotoSansCJKsc-Subset.otf
fi
