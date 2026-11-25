#!/usr/bin/env python3
"""在伪终端里跑一个 TUI 程序，把它画出来的文字抓下来（去掉 ANSI 控制序列）。

用法：python3 capture.py <命令...>
环境变量：CAPTURE_SECS=抓多少秒（默认 12），CAPTURE_OUT=原始字节输出路径。
"""
import os
import pty
import re
import select
import subprocess
import sys
import time

cmd = sys.argv[1:]
master, slave = pty.openpty()
env = dict(os.environ, TERM="xterm-256color", COLUMNS="120", LINES="36")
proc = subprocess.Popen(
    cmd,
    stdin=slave,
    stdout=slave,
    stderr=slave,
    env=env,
    cwd=os.getcwd(),
    preexec_fn=os.setsid,
    close_fds=True,
)
os.close(slave)
buf = b""
deadline = time.time() + float(os.environ.get("CAPTURE_SECS", "12"))
# CAPTURE_KEYS：等待 CAPTURE_WAIT 秒后按顺序敲入（\\n = 回车）
keys = os.environ.get("CAPTURE_KEYS", "")
wait = float(os.environ.get("CAPTURE_WAIT", "8"))
sent = False
while time.time() < deadline:
    if keys and not sent and time.time() > deadline - float(os.environ.get("CAPTURE_SECS", "12")) + wait:
        for part in keys.split("<enter>"):
            os.write(master, part.encode())
            if part != keys.split("<enter>")[-1]:
                os.write(master, b"\r")
            time.sleep(0.6)
        sent = True
    r, _, _ = select.select([master], [], [], 0.3)
    if master in r:
        try:
            chunk = os.read(master, 65536)
        except OSError:
            break
        if not chunk:
            break
        buf += chunk
proc.terminate()
try:
    proc.wait(timeout=5)
except subprocess.TimeoutExpired:
    proc.kill()

out = os.environ.get("CAPTURE_OUT", "capture.raw")
with open(out, "wb") as f:
    f.write(buf)

text = buf.decode("utf-8", "replace")
# CSI / OSC / 字符集切换：换成换行便于逐行看
text = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "\n", text)
text = re.sub(r"\x1b\][^\x07]*\x07", "", text)
text = re.sub(r"\x1b[()][A-Z0-9]|\x1b[=>]", "", text)
lines = [line.rstrip() for line in text.split("\n") if line.strip()]
print(f"=== 抓到 {len(lines)} 行（最后 80 行）===")
print("\n".join(lines[-80:]))
