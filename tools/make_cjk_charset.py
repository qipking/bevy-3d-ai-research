#!/usr/bin/env python3
"""Emit a `--unicodes=` list for pyftsubset: ASCII + CJK punctuation + GB2312 hanzi.

GB2312 level 1 (0xB0A1..0xD7F9) is the 3755 most common simplified hanzi;
level 2 (0xD8A1..0xF7FE) adds 3008 rarer ones. Both are contiguous byte ranges,
so the set is reproducible without a frequency corpus.
"""
import sys

def gb2312_range(lo_hi_byte, hi_hi_byte):
    out = []
    for hi in range(lo_hi_byte, hi_hi_byte + 1):
        for lo in range(0xA1, 0xFF):
            try:
                ch = bytes([hi, lo]).decode("gb2312")
            except UnicodeDecodeError:
                continue
            out.append(ch)
    return out

def main():
    which = sys.argv[1] if len(sys.argv) > 1 else "common"
    chars = set()
    chars.update(chr(c) for c in range(0x20, 0x7F))     # ASCII
    chars.update(chr(c) for c in range(0x00A0, 0x100))  # Latin-1
    chars.update(chr(c) for c in range(0x2000, 0x2070)) # general punctuation
    chars.update(chr(c) for c in range(0x3000, 0x3040)) # CJK punct
    chars.update(chr(c) for c in range(0xFF00, 0xFFF0)) # fullwidth forms
    chars.update("¥·—…‘’“”→←↑↓×÷≤≥±°")
    if which == "common":
        chars.update(gb2312_range(0xB0, 0xD7))          # 3755 hanzi
    else:
        chars.update(gb2312_range(0xB0, 0xF7))          # + level 2
    cps = sorted(ord(c) for c in chars)
    # collapse into ranges
    parts, start, prev = [], cps[0], cps[0]
    for cp in cps[1:]:
        if cp == prev + 1:
            prev = cp
            continue
        parts.append(f"U+{start:04X}" if start == prev else f"U+{start:04X}-{prev:04X}")
        start = prev = cp
    parts.append(f"U+{start:04X}" if start == prev else f"U+{start:04X}-{prev:04X}")
    print(",".join(parts))

if __name__ == "__main__":
    main()
