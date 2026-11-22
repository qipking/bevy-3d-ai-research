# render/

截图证据。每个子目录 = 一次验证，目录名对应那次验证的主题；**较新的在下面这张表里**。

| 目录 | 是什么 | 还有效吗 |
|---|---|---|
| `rf_niri_app/` | 真 app 跑在 niri 布局下（转写列 + composer 列 + 相机） | 最近一次，参考 |
| `rf_niri/` | 平面 niri 平铺原型 | 历史 |
| `rf_ring/` | 圆柱总览 | 历史（已暂缓） |
| `rf_picker*/`、`rf_tree/`、`rf_overlay/` | picker / 树 / 浮层 | 历史 |
| `rf_frame*/`、`rf_overlay/` | 帧布局与浮层对照 | 历史 |
| `probe/`、`shots*/` | 遮挡排查等一次性探针 | 历史 |

生成方式见各 crate 的 `examples/`，环境变量 `RF_SHOT_DIR` 指向对应目录。

> 2026-11-25：用户指示**不再做视觉研究**，这里的图只作记录，不再新增。
