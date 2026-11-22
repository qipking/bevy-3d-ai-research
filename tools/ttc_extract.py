#!/usr/bin/env python3
"""Extract one face from a TrueType Collection (.ttc) into a standalone .ttf/.otf.

Bevy's font loader only accepts `.ttf`/`.otf` extensions (bevy_text/font_loader.rs),
so a system CJK collection has to be split first. Pure stdlib, no fonttools.

usage: ttc_extract.py <font.ttc> <out.ttf> [--list | --pick-sc | --index N]
"""
import struct, sys, os

def read_u32(b, o): return struct.unpack_from(">I", b, o)[0]
def read_u16(b, o): return struct.unpack_from(">H", b, o)[0]

def faces(buf):
    if buf[:4] != b"ttcf":
        return [0]
    n = read_u32(buf, 8)
    return [read_u32(buf, 12 + 4 * i) for i in range(n)]

def sfnt_tables(buf, off):
    num = read_u16(buf, off + 4)
    recs = []
    for i in range(num):
        o = off + 12 + 16 * i
        tag = buf[o:o+4]
        chk, toff, tlen = struct.unpack_from(">III", buf, o + 4)
        recs.append((tag, chk, toff, tlen))
    return recs

def family_name(buf, off):
    recs = dict((t[0], t[2:]) for t in sfnt_tables(buf, off))
    if b"name" not in recs:
        return "?"
    toff, tlen = recs[b"name"]
    fmt, count, str_off = struct.unpack_from(">HHH", buf, toff)
    out = None
    for i in range(count):
        o = toff + 6 + 12 * i
        pid, eid, lid, nid, length, noff = struct.unpack_from(">HHHHHH", buf, o)
        if nid not in (1, 16):
            continue
        raw = buf[toff + str_off + noff: toff + str_off + noff + length]
        try:
            s = raw.decode("utf-16-be") if pid == 3 else raw.decode("latin-1")
        except Exception:
            continue
        if nid == 16 or out is None:
            out = s
    return out or "?"

def extract(buf, off, out_path):
    recs = sfnt_tables(buf, off)
    num = len(recs)
    # new layout: header + records, then tables 4-byte aligned
    header_len = 12 + 16 * num
    data = bytearray()
    new_recs = []
    cur = header_len
    # searchRange etc. (valid but not critical for parsers)
    es = 0
    while (1 << (es + 1)) <= num:
        es += 1
    sr = (1 << es) * 16
    rs = num * 16 - sr
    for tag, chk, toff, tlen in recs:
        body = buf[toff:toff + tlen]
        new_recs.append((tag, chk, cur, tlen, body))
        pad = (4 - tlen % 4) % 4
        data += body + b"\0" * pad
        cur += tlen + pad
    out = bytearray()
    out += buf[off:off + 4]                      # sfnt version
    out += struct.pack(">HHHH", num, sr, es, rs)
    for tag, chk, toff, tlen, _ in new_recs:
        out += tag + struct.pack(">III", chk, toff, tlen)
    out += data
    with open(out_path, "wb") as f:
        f.write(out)
    return out_path

def main():
    if len(sys.argv) < 3:
        print(__doc__); return 1
    path, out = sys.argv[1], sys.argv[2]
    buf = open(path, "rb").read()
    offs = faces(buf)
    names = [(i, family_name(buf, o)) for i, o in enumerate(offs)]
    if "--list" in sys.argv:
        for i, n in names:
            print(f"{i}: {n}")
        return 0
    idx = 0
    if "--pick-sc" in sys.argv:
        sc = [i for i, n in names if "SC" in n]
        idx = sc[0] if sc else 0
    if "--index" in sys.argv:
        idx = int(sys.argv[sys.argv.index("--index") + 1])
    print(f"faces={names} -> picking {idx} ({names[idx][1]})")
    extract(buf, offs[idx], out)
    print(f"wrote {out} ({os.path.getsize(out)} bytes)")
    return 0

if __name__ == "__main__":
    sys.exit(main())
