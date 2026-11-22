# occlusion_probe

Minimal 3D-panel occlusion rig for `research/occlusion_report.md`.

Scene (same as `rig-feathers`): `Camera3d` at `(0, 0.03, 1.45)` + opaque `StandardMaterial`
cube at `(0.68, -0.18, -0.28)` (behind the panel plane) + one `WorldPanel` / `MirrorRoot`
UI panel (780 px wide, `Overflow::scroll_y()`), laid out off-screen at `left: -20000px`.
Latin-only rows on the built-in default font; no CJK/feathers assets needed.

## Run

```bash
cd research/occlusion_probe
export CARGO_HOME=/var/home/lichi/桌面/research/.cargo-home CARGO_NET_OFFLINE=true
BEVY_ASSET_ROOT=$PWD \
PROBE_PANEL_H=300 PROBE_SCROLL=40 PROBE_SHOT_DIR=$PWD/../render/probe/run300_40 \
  cargo run
```

Env:

| var | meaning |
|---|---|
| `PROBE_PANEL_H` | panel height in px (default 380) |
| `PROBE_SCROLL` | `ScrollPosition.y` set at frame 50 (default 0) |
| `PROBE_SHOT_DIR` | screenshot dir (default `../render/probe`) |
| `PROBE_CUBE_Z` | cube z (default -0.28) |
| `PROBE_FRONT_CUBE` | also spawn an opaque cube in front (z=+0.20), depth-test control |
| `PROBE_TOUR_CAM` | replay the `rig-feathers` camera tour (01..05 shots) |
| `PROBE_CLOBBER_BG` | at frame 90 set the panel root's `BackgroundColor` alpha to 0 — simulates the bubbled `PointerOut` that clears it in `rig-feathers` |

Frames: warmup screenshot 30 (first `Screenshot` request after start is blank — Bevy/llvmpipe
quirk), `a_before` 70, optional clobber 90, `b_after` 120, exit 160.

## Instrumentation

* main-world dump at frames 72/122: panel `ComputedNode.size` / `content_size` /
  `ScrollPosition`, every `PanelNode`'s `Transform`/`GlobalTransform`/`PanelSort` and
  `UNodeMaterial3d` color + clip uniforms, cube's `StandardMaterial.alpha_mode`;
* render-world dump in `RenderSystems::PhaseSort` after `sort_phase_system::<Transparent3d>`:
  every `Transparent3d` item, its `MainEntity`, distance, and whether it belongs to a
  `PanelSort` entity (i.e. whether `sort.rs` may have overridden it).

## Related dirs

* `rig_dbg/` — copy of `rig-feathers` (not the original) with `BackgroundColor` /
  `PointerOver/PointerOut` / pick-index logs; used to find and verify the fix.
* `unhover_row_fix.patch` — one-line guard for `rig-feathers::unhover_row`.
