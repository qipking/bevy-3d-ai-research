//! Niri 式**分裂树**布局：窗口（面板）是树上的叶子，位置由树递归切分得到。
//!
//! 和 `frame.rs` 里旧的 `Placement`（"离底边几行"的绝对槽位）不同，这里没有
//! "上/下/剩余"的概念：整块区域先按 [`Split::Horizontal`] 或
//! [`Split::Vertical`] 切成若干份，每一份可以是叶子（一块面板），也可以再是
//! 一个 split。于是"列里再拆列""列里拆上下"都只是嵌套，不需要新的枚举。
//!
//! 语义照 `reference/niri` 的列/窗口分裂模型（只学语义，独立实现）：
//!
//! * 同层相邻两个子之间恰好留 [`Node::Split::gap`] 的缝；
//! * 每个子沿切分轴的长度由 [`Size`] 决定：
//!   * [`Size::Proportion`]：按权重分**去掉固定尺寸和缝之后剩下的空间**；
//!   * [`Size::Fixed`]：固定的 **UI 像素**（见 [`PX_TO_METERS`]），
//!     输入框 4 格、状态行 2 格这种"几行字"的量就用它；
//! * 用累计坐标算边界、**最后一个子把右/下边贴到区域边上**，吃掉浮点舍入误差，
//!   所以叶子之间既不重叠也不留缝（和 `niri.rs::column_edges` 同一做法）。
//!
//! 一个 split 若没有比例子，剩余空间给**第一个子**（主区域）。这条规则让
//! `default_tree` 的右列（输入 Fixed 4 格 + 状态 Fixed 2 格）仍然铺满整屏：
//! 输入是主区域，状态保持 2 格贴在底部。
//!
//! 3D 坐标：区域用 [`TileRect`]（米，原点在帧中心，y 朝上）。第一块子区域在
//! 上/左，第二块在下/右。

use bevy::prelude::Vec2;

use crate::niri::TileRect;
use crate::panels::PanelId;

/// UI 像素 → 世界米（与 `frame.rs` 一致）。
///
/// [`Size::Fixed`] 和 [`Node::Split::gap`] 的单位都是 **UI 像素**：
/// 面板尺寸、`CELL`、`Node` 尺寸本来就都是像素，用像素写"4 格高"最直观。
pub const PX_TO_METERS: f32 = crate::frame::UNITS_TO_WORLD;

/// 用户手动拆窗时默认留的缝（UI 像素）。
///
/// 默认屏幕的树用 [`NO_GAP`]：三块面板自己带边框和内边距，树里再加缝反而
/// 会让"叶子面积之和 = 整屏"多出误差项。
pub const DEFAULT_GAP: f32 = 6.0;

/// 不留缝。
pub const NO_GAP: f32 = 0.0;

/// 切分方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Split {
    /// 左右切：第一个子在左，第二个在右。
    Horizontal,
    /// 上下切：第一个子在上，第二个在下。
    Vertical,
}

/// 子区域沿切分轴要占多少。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Size {
    /// 相对同层兄弟的比例：权重 / 同层所有比例权重之和。
    ///
    /// 它分的是"区域去掉固定尺寸和 gap 之后剩下的空间"。
    Proportion(f32),
    /// 固定 **UI 像素**（会按 [`PX_TO_METERS`] 换算成米）。
    ///
    /// 4 格输入框 = `4.0 * CELL.y`，2 格状态行 = `2.0 * CELL.y`。
    Fixed(f32),
}

/// 树上的一个子：尺寸约束 + 子树。
#[derive(Clone, Debug, PartialEq)]
pub struct Child {
    pub size: Size,
    pub node: Node,
}

impl Child {
    pub fn new(size: Size, node: Node) -> Self {
        Self { size, node }
    }

    /// 比例子的便捷构造。
    pub fn proportion(weight: f32, node: Node) -> Self {
        Self::new(Size::Proportion(weight), node)
    }

    /// 固定尺寸子的便捷构造（UI 像素）。
    pub fn fixed(px: f32, node: Node) -> Self {
        Self::new(Size::Fixed(px), node)
    }

    /// 叶子子的便捷构造。
    pub fn leaf(size: Size, panel: PanelId) -> Self {
        Self::new(size, Node::leaf(panel))
    }
}

/// 分裂树。
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// 一个切分：按 `direction` 把区域切成 `children`，相邻子之间留 `gap`（UI px）。
    Split {
        direction: Split,
        /// 同层相邻子之间的缝（UI 像素，见 [`PX_TO_METERS`]）。
        gap: f32,
        children: Vec<Child>,
    },
    /// 一块面板。
    Leaf { panel: PanelId },
}

impl Node {
    pub fn leaf(panel: PanelId) -> Self {
        Self::Leaf { panel }
    }

    /// 左右/上下切出一个 split。
    pub fn split(direction: Split, gap: f32, children: Vec<Child>) -> Self {
        Self::Split {
            direction,
            gap,
            children,
        }
    }

    /// 空 split（或只装着空 split 的 split）。
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Leaf { .. } => false,
            Self::Split { children, .. } => {
                children.is_empty() || children.iter().all(|child| child.node.is_empty())
            }
        }
    }

    /// 叶子面板列表（先左/上，后右/下）。
    pub fn leaves(&self) -> Vec<PanelId> {
        match self {
            Self::Leaf { panel } => vec![*panel],
            Self::Split { children, .. } => children
                .iter()
                .flat_map(|child| child.node.leaves())
                .collect(),
        }
    }

    /// 把 `area` 递归切成每个叶子的矩形。
    ///
    /// 保证：同一个 split 的相邻叶子之间恰好是 `gap`；所有叶子既不重叠，
    /// 也不在区域里留缝（最后一个子的远端贴住区域边，吃掉舍入误差）。
    pub fn rects(&self, area: TileRect) -> Vec<(PanelId, TileRect)> {
        match self {
            Self::Leaf { panel } => vec![(*panel, area)],
            Self::Split {
                direction,
                gap,
                children,
            } => {
                if children.is_empty() {
                    return Vec::new();
                }
                let horizontal = matches!(direction, Split::Horizontal);
                let total = if horizontal {
                    area.size.x.max(0.0)
                } else {
                    area.size.y.max(0.0)
                };
                let count = children.len();
                // 缝不能把区域挤成负数：总缝最多等于区域，剩下的才是子区域的空间。
                let gap = gap.sanitized() * PX_TO_METERS;
                let gap = if count > 1 {
                    gap.min(total / (count - 1) as f32)
                } else {
                    0.0
                };
                let gaps = gap * (count.saturating_sub(1)) as f32;
                let available = (total - gaps).max(0.0);

                let fixed_total: f32 = children
                    .iter()
                    .filter_map(|child| match child.size {
                        Size::Fixed(px) => Some(px.sanitized() * PX_TO_METERS),
                        Size::Proportion(_) => None,
                    })
                    .sum();
                let proportion_total: f32 = children
                    .iter()
                    .filter_map(|child| match child.size {
                        Size::Proportion(weight) => Some(weight.sanitized()),
                        Size::Fixed(_) => None,
                    })
                    .sum();

                let mut lengths = vec![0.0f32; count];
                if proportion_total > 0.0 {
                    // 固定子先拿，比例子分剩下的；固定子总和超了就整体按比例缩。
                    let fixed_fit = fixed_total.min(available);
                    let fixed_scale = if fixed_total > 0.0 {
                        fixed_fit / fixed_total
                    } else {
                        1.0
                    };
                    let leftover = (available - fixed_fit).max(0.0);
                    for (index, child) in children.iter().enumerate() {
                        lengths[index] = match child.size {
                            Size::Fixed(px) => px.sanitized() * PX_TO_METERS * fixed_scale,
                            Size::Proportion(weight) => {
                                leftover * weight.sanitized() / proportion_total
                            }
                        };
                    }
                } else {
                    // 没有比例子：第一个子是主区域，吃掉剩余；其余固定子保持精确。
                    let fixed_scale = if fixed_total > available && fixed_total > 0.0 {
                        available / fixed_total
                    } else {
                        1.0
                    };
                    for (index, child) in children.iter().enumerate() {
                        if let Size::Fixed(px) = child.size {
                            lengths[index] = px.sanitized() * PX_TO_METERS * fixed_scale;
                        }
                    }
                    let used: f32 = lengths.iter().sum();
                    lengths[0] += (available - used).max(0.0);
                }

                let near = if horizontal {
                    area.center.x - area.size.x * 0.5
                } else {
                    area.center.y + area.size.y * 0.5
                };
                let far = if horizontal {
                    area.center.x + area.size.x * 0.5
                } else {
                    area.center.y - area.size.y * 0.5
                };
                let mut cursor = near;
                let mut out = Vec::new();
                for (index, child) in children.iter().enumerate() {
                    // 最后一个子贴住远端：浮点误差不会在区域边上留一条缝。
                    let end = if index + 1 == count {
                        far
                    } else if horizontal {
                        cursor + lengths[index]
                    } else {
                        cursor - lengths[index]
                    };
                    let child_area = if horizontal {
                        TileRect {
                            center: Vec2::new((cursor + end) * 0.5, area.center.y),
                            size: Vec2::new((end - cursor).max(0.0), area.size.y),
                        }
                    } else {
                        TileRect {
                            center: Vec2::new(area.center.x, (cursor + end) * 0.5),
                            size: Vec2::new(area.size.x, (cursor - end).max(0.0)),
                        }
                    };
                    out.extend(child.node.rects(child_area));
                    cursor = if horizontal {
                        end + gap
                    } else {
                        end - gap
                    };
                }
                out
            }
        }
    }

    /// 整理树，让它更适合继续拆：
    ///
    /// 1. 递归整理每个子；
    /// 2. 丢掉空的子 split（[`Node::is_empty`]）；
    /// 3. 只剩一个子的 split 摊平成那个子；
    /// 4. 同层比例权重归一化（和为 1）；全为 0/NaN 时改成等分。
    pub fn normalize(&mut self) {
        let Self::Split { children, .. } = self else {
            return;
        };
        for child in children.iter_mut() {
            child.node.normalize();
        }
        children.retain(|child| !child.node.is_empty());

        let proportions: Vec<usize> = children
            .iter()
            .enumerate()
            .filter_map(|(index, child)| {
                matches!(child.size, Size::Proportion(_)).then_some(index)
            })
            .collect();
        if !proportions.is_empty() {
            let total: f32 = proportions
                .iter()
                .filter_map(|index| match children[*index].size {
                    Size::Proportion(weight) => Some(weight.sanitized()),
                    Size::Fixed(_) => None,
                })
                .sum();
            for index in &proportions {
                let Size::Proportion(weight) = &mut children[*index].size else {
                    continue;
                };
                *weight = if total > 0.0 && total.is_finite() {
                    weight.sanitized() / total
                } else {
                    1.0 / proportions.len() as f32
                };
            }
        }

        if children.len() == 1 {
            let child = children.pop().expect("刚检查过只有一个子");
            *self = child.node;
        }
    }

    /// 把一个叶子换成"原叶子 + 新叶子"的 split。这是"窗口能拆分"的最小操作。
    ///
    /// 方向由调用方定；新 split 用 [`DEFAULT_GAP`] 留缝。找到 `panel` 返回
    /// `true`（即使 `new_panel` 和 `panel` 相同，也照拆），找不到返回 `false`。
    pub fn split_leaf(&mut self, panel: PanelId, direction: Split, new_panel: PanelId) -> bool {
        match self {
            Self::Leaf { panel: found } if *found == panel => {
                let original = *found;
                *self = Self::split(
                    direction,
                    DEFAULT_GAP,
                    vec![
                        Child::proportion(1.0, Node::leaf(original)),
                        Child::proportion(1.0, Node::leaf(new_panel)),
                    ],
                );
                true
            }
            Self::Leaf { .. } => false,
            Self::Split { children, .. } => children
                .iter_mut()
                .any(|child| child.node.split_leaf(panel, direction, new_panel)),
        }
    }
}

/// `f32` 的 `max` 不会滤掉 NaN；这里统一把 NaN/∞ 变成 0。
trait Sanitized {
    fn sanitized(self) -> f32;
}

impl Sanitized for f32 {
    fn sanitized(self) -> f32 {
        if self.is_finite() {
            self.max(0.0)
        } else {
            0.0
        }
    }
}

/// 默认屏幕的树（`RF_NIRI_LAYOUT` 未打开时用）。
///
/// 形状对应 niri 的"转写一大列 + 右边输入列"：
///
/// ```text
/// Horizontal
/// ├─ Transcript   Proportion 2   ← 左 2/3
/// └─ Vertical                   ← 右 1/3
///    ├─ Input      Fixed 4 格   ← 提示行 + 键位提示行 + 面板装饰
///    └─ Status     Fixed 2 格   ← 细条里居中一行
/// ```
///
/// 数值理由：
/// * 转写是主区域，给 2 份；输入列 1 份，和 niri 的预设列宽 2/3 + 1/3 对齐。
/// * 输入 4 格 = [`crate::transcript::ROW_HEIGHT`] × 4：两行文字（提示 + 键位
///   提示）加内边距/边框；状态 2 格：一行字在细条里居中。
/// * 右列没有比例子，按 [`Node::rects`] 的规则由第一个子（输入）吃掉剩余高度，
///   状态固定保持 2 格贴底。
/// * 树里 gap = [`NO_GAP`]：面板自己有边框和内边距，默认屏幕不需要额外缝，
///   叶子面积正好铺满整屏。
pub fn default_tree() -> Node {
    use crate::transcript::ROW_HEIGHT;

    Node::split(
        Split::Horizontal,
        NO_GAP,
        vec![
            Child::proportion(2.0, Node::leaf(PanelId::Transcript)),
            Child::new(
                Size::Proportion(1.0),
                Node::split(
                    Split::Vertical,
                    NO_GAP,
                    vec![
                        Child::fixed(4.0 * ROW_HEIGHT, Node::leaf(PanelId::Input)),
                        Child::fixed(2.0 * ROW_HEIGHT, Node::leaf(PanelId::Status)),
                    ],
                ),
            ),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::Vec2;

    /// 测试区域：1.2 m × 0.8 m，中心不在原点，顺便验证平移。
    fn area() -> TileRect {
        TileRect {
            center: Vec2::new(0.7, -0.3),
            size: Vec2::new(1.2, 0.8),
        }
    }

    fn rects(node: &Node) -> Vec<(PanelId, TileRect)> {
        node.rects(area())
    }

    fn left(rect: &TileRect) -> f32 {
        rect.center.x - rect.size.x * 0.5
    }

    fn right(rect: &TileRect) -> f32 {
        rect.center.x + rect.size.x * 0.5
    }

    fn top(rect: &TileRect) -> f32 {
        rect.center.y + rect.size.y * 0.5
    }

    fn bottom(rect: &TileRect) -> f32 {
        rect.center.y - rect.size.y * 0.5
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    fn leaf(panel: PanelId) -> Node {
        Node::leaf(panel)
    }

    // 1. 单叶子 = 整个区域。
    #[test]
    fn a_single_leaf_takes_the_whole_area() {
        let found = rects(&leaf(PanelId::Transcript));
        assert_eq!(found.len(), 1);
        let (panel, rect) = found[0];
        assert_eq!(panel, PanelId::Transcript);
        assert_eq!(rect, area());
    }

    // 2. 两个比例兄弟按 1:2 分（而不是各半）。
    #[test]
    fn two_proportion_siblings_split_by_their_weight() {
        let node = Node::split(
            Split::Horizontal,
            NO_GAP,
            vec![
                Child::proportion(1.0, leaf(PanelId::Transcript)),
                Child::proportion(2.0, leaf(PanelId::Status)),
            ],
        );
        let found = rects(&node);
        assert_eq!(found.len(), 2);
        assert!(close(found[0].1.size.x, 0.4), "1/3 宽");
        assert!(close(found[1].1.size.x, 0.8), "2/3 宽");
        assert!(close(right(&found[0].1), left(&found[1].1)), "兄弟贴在一起");
        assert!(close(left(&found[0].1), left(&area())));
        assert!(close(right(&found[1].1), right(&area())));
    }

    // 3. 固定 + 比例混合：固定子先拿像素，比例子吃剩下的。
    #[test]
    fn fixed_and_proportion_sizes_mix() {
        let fixed_px = 100.0;
        let node = Node::split(
            Split::Horizontal,
            NO_GAP,
            vec![
                Child::fixed(fixed_px, leaf(PanelId::Status)),
                Child::proportion(1.0, leaf(PanelId::Transcript)),
            ],
        );
        let found = rects(&node);
        assert!(close(found[0].1.size.x, fixed_px * PX_TO_METERS));
        assert!(close(
            found[1].1.size.x,
            area().size.x - fixed_px * PX_TO_METERS
        ));
        assert!(close(right(&found[1].1), right(&area())));
    }

    // 4. 嵌套：右列里再上下拆（"列里拆上下"）。
    #[test]
    fn a_nested_column_splits_again() {
        let node = Node::split(
            Split::Horizontal,
            NO_GAP,
            vec![
                Child::proportion(2.0, leaf(PanelId::Transcript)),
                Child::new(
                    Size::Proportion(1.0),
                    Node::split(
                        Split::Vertical,
                        NO_GAP,
                        vec![
                            Child::fixed(90.0, leaf(PanelId::Input)),
                            Child::fixed(36.0, leaf(PanelId::Status)),
                        ],
                    ),
                ),
            ],
        );
        let found = rects(&node);
        assert_eq!(found.len(), 3);
        let transcript = found.iter().find(|(id, _)| *id == PanelId::Transcript).unwrap().1;
        let input = found.iter().find(|(id, _)| *id == PanelId::Input).unwrap().1;
        let status = found.iter().find(|(id, _)| *id == PanelId::Status).unwrap().1;
        // 左右两列：转写 2/3，右列 1/3。
        assert!(close(transcript.size.x, area().size.x * 2.0 / 3.0));
        assert!(close(right(&transcript), left(&input)));
        assert!(close(right(&input), right(&area())));
        // 右列上下：状态固定 36 px 贴底，输入是第一个子，吃掉其余高度。
        assert!(close(status.size.y, 36.0 * PX_TO_METERS));
        assert!(
            close(
                input.size.y,
                area().size.y - 36.0 * PX_TO_METERS
            ),
            "输入是主区域，吃剩余高度"
        );
        assert!(close(top(&input), top(&area())));
        assert!(close(bottom(&input), top(&status)));
        assert!(close(bottom(&status), bottom(&area())));
    }

    // 5. gap 生效：相邻边正好差 gap，总宽 + 总缝 = 区域宽，最后一边贴住区域。
    #[test]
    fn gaps_are_exact_between_siblings() {
        let gap_px = 20.0;
        let node = Node::split(
            Split::Horizontal,
            gap_px,
            vec![
                Child::proportion(1.0, leaf(PanelId::Transcript)),
                Child::proportion(1.0, leaf(PanelId::Status)),
                Child::proportion(1.0, leaf(PanelId::Input)),
            ],
        );
        let found = rects(&node);
        assert_eq!(found.len(), 3);
        let gap = gap_px * PX_TO_METERS;
        for pair in found.windows(2) {
            assert!(close(left(&pair[1].1) - right(&pair[0].1), gap), "恰好留 gap");
        }
        let widths: f32 = found.iter().map(|(_, rect)| rect.size.x).sum();
        assert!(close(widths + 2.0 * gap, area().size.x), "叶子 + 缝 = 区域");
        assert!(close(left(&found[0].1), left(&area())));
        assert!(close(right(&found[2].1), right(&area())));
    }

    // 6. 竖直方向同样：累计坐标 + 最后一个子吃舍入，三块严丝合缝。
    #[test]
    fn vertical_children_tile_the_area_without_a_seam() {
        let node = Node::split(
            Split::Vertical,
            NO_GAP,
            vec![
                Child::proportion(1.0, leaf(PanelId::Status)),
                Child::proportion(1.0, leaf(PanelId::Input)),
                Child::proportion(1.0, leaf(PanelId::Transcript)),
            ],
        );
        let found = rects(&node);
        assert_eq!(found.len(), 3);
        for pair in found.windows(2) {
            assert!(close(bottom(&pair[0].1), top(&pair[1].1)), "上下相接");
        }
        assert!(close(top(&found[0].1), top(&area())));
        assert!(close(bottom(&found[2].1), bottom(&area())));
        let heights: f32 = found.iter().map(|(_, rect)| rect.size.y).sum();
        assert!(close(heights, area().size.y), "高度之和精确等于区域");
    }

    // 7. normalize：空 split 折叠掉。
    #[test]
    fn normalize_folds_an_empty_split_away() {
        let mut node = Node::split(
            Split::Horizontal,
            NO_GAP,
            vec![
                Child::proportion(1.0, Node::split(Split::Vertical, NO_GAP, Vec::new())),
                Child::proportion(1.0, leaf(PanelId::Transcript)),
            ],
        );
        node.normalize();
        assert_eq!(node, leaf(PanelId::Transcript), "只剩一个子时摊平成叶子");
    }

    // 8. normalize：单子 split 摊平。
    #[test]
    fn normalize_flattens_a_single_child_split() {
        let mut node = Node::split(
            Split::Vertical,
            DEFAULT_GAP,
            vec![Child::proportion(3.0, leaf(PanelId::Input))],
        );
        node.normalize();
        assert_eq!(node, leaf(PanelId::Input));
    }

    // 9. normalize：同层比例归一化（和为 1）。
    #[test]
    fn normalize_rebalances_proportions() {
        let mut node = Node::split(
            Split::Horizontal,
            NO_GAP,
            vec![
                Child::proportion(2.0, leaf(PanelId::Transcript)),
                Child::proportion(6.0, leaf(PanelId::Status)),
            ],
        );
        node.normalize();
        let Node::Split { children, .. } = &node else {
            panic!("两个子不该摊平");
        };
        assert_eq!(children[0].size, Size::Proportion(0.25));
        assert_eq!(children[1].size, Size::Proportion(0.75));
        // 0 权重全为 0 时退化成等分，不会除以 0。
        let mut zero = Node::split(
            Split::Horizontal,
            NO_GAP,
            vec![
                Child::proportion(0.0, leaf(PanelId::Transcript)),
                Child::proportion(0.0, leaf(PanelId::Status)),
            ],
        );
        zero.normalize();
        let Node::Split { children, .. } = &zero else {
            panic!("两个子不该摊平");
        };
        assert_eq!(children[0].size, Size::Proportion(0.5));
        assert_eq!(children[1].size, Size::Proportion(0.5));
    }

    // 10. split_leaf：Transcript 拆成左右两半，两半都拿到 PanelId。
    #[test]
    fn split_leaf_turns_transcript_into_two_panels() {
        let mut node = leaf(PanelId::Transcript);
        assert!(node.split_leaf(PanelId::Transcript, Split::Horizontal, PanelId::Preview));
        let found = rects(&node);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, PanelId::Transcript);
        assert_eq!(found[1].0, PanelId::Preview);
        let gap = DEFAULT_GAP * PX_TO_METERS;
        assert!(close(found[0].1.size.x, found[1].1.size.x), "两半等宽");
        assert!(close(left(&found[1].1) - right(&found[0].1), gap));
        assert!(close(
            found[0].1.size.x + found[1].1.size.x + gap,
            area().size.x
        ));
        // 不存在的面板拆不动。
        assert!(!node.split_leaf(PanelId::Status, Split::Vertical, PanelId::List));
    }

    // 11. 列里再拆列：下半个区域横向分成两块，两块的 y 范围与父区域一致。
    #[test]
    fn a_column_can_split_into_columns_again() {
        let node = Node::split(
            Split::Vertical,
            NO_GAP,
            vec![
                Child::proportion(1.0, leaf(PanelId::Transcript)),
                Child::new(
                    Size::Proportion(1.0),
                    Node::split(
                        Split::Horizontal,
                        NO_GAP,
                        vec![
                            Child::proportion(1.0, leaf(PanelId::Input)),
                            Child::proportion(1.0, leaf(PanelId::Status)),
                        ],
                    ),
                ),
            ],
        );
        let found = rects(&node);
        assert_eq!(found.len(), 3);
        let transcript = found.iter().find(|(id, _)| *id == PanelId::Transcript).unwrap().1;
        let input = found.iter().find(|(id, _)| *id == PanelId::Input).unwrap().1;
        let status = found.iter().find(|(id, _)| *id == PanelId::Status).unwrap().1;
        // 上下两半平分高度，转写在上。
        assert!(close(transcript.size.y, area().size.y * 0.5));
        assert!(close(top(&transcript), top(&area())));
        assert!(close(bottom(&transcript), top(&input)));
        // 下半个区域里再左右拆：输入/状态同高、同一条水平线。
        assert!(close(input.size.x, status.size.x));
        assert!(close(top(&input), top(&status)));
        assert!(close(bottom(&input), bottom(&area())));
        assert!(close(right(&input), left(&status)));
    }

    // 12. split_leaf 能找到嵌套里的叶子（列里再拆列）。
    #[test]
    fn split_leaf_reaches_into_nested_splits() {
        let mut node = Node::split(
            Split::Horizontal,
            NO_GAP,
            vec![
                Child::proportion(1.0, leaf(PanelId::Transcript)),
                Child::proportion(1.0, leaf(PanelId::Input)),
            ],
        );
        assert!(node.split_leaf(PanelId::Input, Split::Vertical, PanelId::Status));
        let found = rects(&node);
        assert_eq!(found.len(), 3);
        let input = found.iter().find(|(id, _)| *id == PanelId::Input).unwrap().1;
        let status = found.iter().find(|(id, _)| *id == PanelId::Status).unwrap().1;
        assert!(close(left(&input), left(&status)), "同一列左右对齐");
        assert!(close(top(&input), top(&area())));
    }

    // 13. 默认树铺满整屏（叶子面积之和 = 区域面积）。
    #[test]
    fn the_default_tree_covers_the_screen() {
        let node = default_tree();
        let found = node.rects(area());
        assert_eq!(found.len(), 3);
        let mut ids: Vec<PanelId> = found.iter().map(|(id, _)| *id).collect();
        ids.sort_by_key(|id| format!("{id:?}"));
        assert_eq!(
            ids,
            vec![PanelId::Input, PanelId::Status, PanelId::Transcript]
        );
        let leaves: f32 = found
            .iter()
            .map(|(_, rect)| rect.size.x * rect.size.y)
            .sum();
        let screen = area().size.x * area().size.y;
        assert!(
            (leaves - screen).abs() < 1e-3,
            "叶子面积 {leaves} != 区域 {screen}"
        );
        // 左 2/3、右 1/3；状态在右下角，输入占右列其余高度。
        let transcript = found.iter().find(|(id, _)| *id == PanelId::Transcript).unwrap().1;
        let input = found.iter().find(|(id, _)| *id == PanelId::Input).unwrap().1;
        let status = found.iter().find(|(id, _)| *id == PanelId::Status).unwrap().1;
        assert!(close(transcript.size.x, area().size.x * 2.0 / 3.0));
        assert!(close(status.size.y, 2.0 * crate::transcript::ROW_HEIGHT * PX_TO_METERS));
        assert!(close(bottom(&status), bottom(&area())));
        assert!(close(bottom(&input), top(&status)));
    }

    // 14. 固定子总和超过区域时整体缩小，仍然不越界、不重叠。
    #[test]
    fn fixed_children_that_do_not_fit_are_scaled_down() {
        let node = Node::split(
            Split::Horizontal,
            NO_GAP,
            vec![
                Child::fixed(1000.0, leaf(PanelId::Transcript)),
                Child::fixed(1000.0, leaf(PanelId::Status)),
            ],
        );
        let found = rects(&node);
        assert_eq!(found.len(), 2);
        assert!(close(found[0].1.size.x, found[1].1.size.x), "等比例缩");
        assert!(close(left(&found[0].1), left(&area())));
        assert!(close(right(&found[1].1), right(&area())));
        assert!(close(right(&found[0].1), left(&found[1].1)));
    }

    // 15. NaN / 负的尺寸不会把子区域甩到区域外。
    #[test]
    fn bad_sizes_are_sanitized() {
        let node = Node::split(
            Split::Horizontal,
            NO_GAP,
            vec![
                Child::new(Size::Fixed(f32::NAN), leaf(PanelId::Transcript)),
                Child::proportion(1.0, leaf(PanelId::Status)),
            ],
        );
        let found = rects(&node);
        assert_eq!(found[0].1.size.x, 0.0, "NaN 固定尺寸当成 0");
        assert!(close(left(&found[0].1), left(&area())));
        assert!(close(found[1].1.size.x, area().size.x), "比例子拿到全部宽度");
        assert!(close(right(&found[1].1), right(&area())));
    }
}
