# rig-official：官方 `rig` 的本地副本（用来对照）

**这是 0xPlaygrounds/rig 的官方终端程序**（crates.io 上的 `rig 0.47.0`），也就是我们
`rig_harness_in_3d` 要移植的那个界面。仓库根目录的 `run-official.sh` 会跑它。

> 别和 [openrig.dev](https://openrig.dev) 混淆：那是 **另一个项目**（`mvschwarz/openrig`，
> Apache-2.0，TypeScript + Node + tmux），做的是"一批 agent 的团队调度仪表盘"，
> 不用 `rig-harness`/`rig-tui` 这些 crate，和本仓库无关。

## 这个目录里有什么

| 文件 | 说明 |
|---|---|
| `src/`、`Cargo.toml`、`img/` 等 | 从 registry 原样复制过来的 crate 源码，**能直接 `cargo build --bin rig`** |
| `capture.py` | 在伪终端里跑 TUI 并把屏幕文字抓下来（去 ANSI），用来跟我们的实现逐字对照 |

上面那些源码**没有进 git**（见 `.gitignore` 的 `experiments/rig-official/*`）——它们可由下面
一条命令重新生成，没必要在仓库里存一份。

## 重新生成

```sh
cd /var/home/lichi/桌面/research
cp -r .cargo-home/registry/src/index.crates.io-*/rig-0.47.0/. experiments/rig-official/
rm -f experiments/rig-official/.cargo-ok
cd experiments/rig-official
CARGO_HOME=/var/home/lichi/桌面/research/.cargo-home cargo build --offline --bin rig
```

## 跑

```sh
/var/home/lichi/桌面/research/run-official.sh      # 真终端里进 TUI
```

或者非交互地抓屏（不带 TTY 也能看它画了什么）：

```sh
cd /var/home/lichi/桌面/research/experiments/rig-official
RIG_HOME=/var/home/lichi/桌面/research/.rig-home CARGO_HOME=/var/home/lichi/桌面/research/.cargo-home \
CAPTURE_SECS=16 CAPTURE_WAIT=6 CAPTURE_KEYS='/he' python3 capture.py ./target/debug/rig
```

抓到的真实界面记录在 [`docs/reports/official-tui.md`](../../docs/reports/official-tui.md)。
