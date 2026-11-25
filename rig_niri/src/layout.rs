//! Niri 式**分裂树**布局：窗口（面板）是树上的叶子，位置由树递归切分得到。
//!
//! 和 `frame.rs` 里旧的 `Placement`（"离底边几行"的绝对槽位）不同，这里没有
//! "上/下/剩余"的概念：整块区域先按 [`Split::Horizontal`] 或
//! [`Split::Vertical`] 切成若干份，每一份可以是叶子（一块面板），也可以再是
//! 一个 split。于是"列里再拆列""列里拆上下"都只是嵌套，不需要新的枚举。
//!
//! 还有一种 [`Node::Container`]：一块容器面板包着一段区域（picker 那一列的
//! 底色/边框）。容器**不是叶子**，[`Node::rects`] 只返回真正的面板叶子；
//! [`Node::containers`] 单独给容器区域，绘制时它垫在内部面板下面。
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
//! 只有固定子的列也能铺满：第一个子吃掉剩余，后面的固定子保持精确。
//! `default_tree` 的右列则把输入写成比例子（补全 tile 的 0/6 行是固定子，
//! 输入拿剩下的），所以补全打开/关闭时右列总高都精确不变。
//!
//! 条带最右边加一列（picker）用 [`Node::append_column`]：新列占 1/3，已有列
//! 按同一因子等比缩小；[`Node::remove_column`] 摘掉它，树逐字节复原。
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
    /// 一块**容器面板**包着一段区域：容器自己画底色/边框，内部再排子节点。
    ///
    /// 容器**不是叶子**：[`Node::rects`] 只返回真正的面板叶子（容器和它们
    /// 几何重叠，靠绘制层压在后面），[`Node::containers`] 单独返回容器的
    /// 区域。picker 那一列就是"`Picker` 容器 + 列内的 `List` / `Preview`"。
    Container {
        /// 容器面板。
        panel: PanelId,
        /// 容器内部的分裂树。
        node: Box<Node>,
    },
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

    /// 一块容器面板包着 `node`（见 [`Node::Container`]）。
    pub fn container(panel: PanelId, node: Node) -> Self {
        Self::Container {
            panel,
            node: Box::new(node),
        }
    }

    /// 空 split（或只装着空 split 的 split）。
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Leaf { .. } => false,
            Self::Container { node, .. } => node.is_empty(),
            Self::Split { children, .. } => {
                children.is_empty() || children.iter().all(|child| child.node.is_empty())
            }
        }
    }

    /// 叶子面板列表（先左/上，后右/下）；容器不算叶子。
    pub fn leaves(&self) -> Vec<PanelId> {
        match self {
            Self::Leaf { panel } => vec![*panel],
            Self::Container { node, .. } => node.leaves(),
            Self::Split { children, .. } => children
                .iter()
                .flat_map(|child| child.node.leaves())
                .collect(),
        }
    }

    /// 把 `area` 递归切成每个**叶子**的矩形（容器不算，见 [`Node::containers`]）。
    ///
    /// 保证：同一个 split 的相邻叶子之间恰好是 `gap`；所有叶子既不重叠，
    /// 也不在区域里留缝（最后一个子的远端贴住区域边，吃掉舍入误差）。
    pub fn rects(&self, area: TileRect) -> Vec<(PanelId, TileRect)> {
        self.placements(area)
            .into_iter()
            .filter_map(|(panel, rect, container)| (!container).then_some((panel, rect)))
            .collect()
    }

    /// 容器面板的区域（和它内部的叶子重叠，只是绘制层更低）。
    pub fn containers(&self, area: TileRect) -> Vec<(PanelId, TileRect)> {
        self.placements(area)
            .into_iter()
            .filter_map(|(panel, rect, container)| container.then_some((panel, rect)))
            .collect()
    }

    /// 递归切分，把容器和叶子都按先后顺序收进来。
    ///
    /// 返回值第三项是"这是不是容器"：容器永远在它内部的面板之前出现。
    pub fn placements(&self, area: TileRect) -> Vec<(PanelId, TileRect, bool)> {
        let mut out = Vec::new();
        self.walk(area, &mut out);
        out
    }

    /// [`Node::placements`] 的递归体。
    fn walk(&self, area: TileRect, out: &mut Vec<(PanelId, TileRect, bool)>) {
        match self {
            Self::Leaf { panel } => out.push((*panel, area, false)),
            Self::Container { panel, node } => {
                out.push((*panel, area, true));
                node.walk(area, out);
            }
            Self::Split {
                direction,
                gap,
                children,
            } => {
                if children.is_empty() {
                    return;
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

                // 防护：固定尺寸是 UI 像素，不可能小于 1px。小于 1 基本就是把米当像素
                // 传进来了（例如 `Child::fixed(36.0 * PX_TO_METERS)`），那样固定子会
                // 塌成 ~0，同层"第一个子吃剩余"会把位置全打乱。
                for child in children.iter() {
                    if let Size::Fixed(px) = child.size
                        && px.sanitized() > 0.0
                        && px.sanitized() < 1.0
                    {
                        bevy::log::warn!(
                            "layout: 固定尺寸 {px} px 小于 1，疑似把米当像素传了（Fixed 要写像素）"
                        );
                    }
                }
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
                    child.node.walk(child_area, out);
                    cursor = if horizontal {
                        end + gap
                    } else {
                        end - gap
                    };
                }
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
        if let Self::Container { node, .. } = self {
            node.normalize();
            return;
        }
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
    /// 方向由调用方定；新 split 用 [`DEFAULT_GAP`] 留缝、两个子等宽。找到
    /// `panel` 且 `new_panel` 还不在树上时返回 `true`；找不到、或者新面板已经
    /// 存在时返回 `false`（同一块面板出现两次的话 [`crate::frame::PanelRoots`]
    /// 没法把两个叶子映到各自的实体，所以不允许重复）。
    pub fn split_leaf(&mut self, panel: PanelId, direction: Split, new_panel: PanelId) -> bool {
        self.split_leaf_sized(
            panel,
            direction,
            new_panel,
            DEFAULT_GAP,
            Size::Proportion(1.0),
            Size::Proportion(1.0),
        )
    }

    /// 同 [`Node::split_leaf`]，但缝和两个子的尺寸由调用方定。
    ///
    /// picker 的 preview 用 [`NO_GAP`] + `2:1` 把一列竖着切成"列表 / preview"：
    /// 同一列内部不需要缝（两个面板自己带边框），2:1 是和 niri 的列内预设对齐。
    pub fn split_leaf_sized(
        &mut self,
        panel: PanelId,
        direction: Split,
        new_panel: PanelId,
        gap: f32,
        original: Size,
        added: Size,
    ) -> bool {
        // 重复触发（已经拆过）要安全地什么也不做：树保持原样。
        if new_panel == panel || self.leaves().contains(&new_panel) {
            return false;
        }
        match self {
            Self::Leaf { panel: found } if *found == panel => {
                let original_panel = *found;
                *self = Self::split(
                    direction,
                    gap,
                    vec![
                        Child::new(original, Node::leaf(original_panel)),
                        Child::new(added, Node::leaf(new_panel)),
                    ],
                );
                true
            }
            Self::Leaf { .. } => false,
            Self::Container { node, .. } => {
                node.split_leaf_sized(panel, direction, new_panel, gap, original, added)
            }
            Self::Split { children, .. } => children.iter_mut().any(|child| {
                child
                    .node
                    .split_leaf_sized(panel, direction, new_panel, gap, original, added)
            }),
        }
    }

    /// `panel` 这个叶子的尺寸约束，找不到返回 `None`。
    ///
    /// 尺寸挂在父 [`Child`] 上（叶子自身没有尺寸），所以从根开始找"装着这个
    /// 叶子的那个子"。根正好是叶子时返回 `None`（根没有父，也就没有尺寸）。
    pub fn leaf_size(&self, panel: PanelId) -> Option<Size> {
        match self {
            Self::Leaf { .. } => None,
            Self::Container { node, .. } => node.leaf_size(panel),
            Self::Split { children, .. } => {
                for child in children {
                    if let Self::Leaf { panel: found } = &child.node {
                        if *found == panel {
                            return Some(child.size);
                        }
                    }
                    if let Some(size) = child.node.leaf_size(panel) {
                        return Some(size);
                    }
                }
                None
            }
        }
    }

    /// 改 `panel` 这个叶子的尺寸约束；找到并真的改了返回 `true`。
    ///
    /// 补全 tile 的开合就走这里：关着时 `Fixed(0.0)`（不占高度），打开时
    /// `Fixed(6 行)`。尺寸没变时不写，免得每帧都把 [`crate::frame::FrameTree`]
    /// 标脏。
    pub fn set_leaf_size(&mut self, panel: PanelId, size: Size) -> bool {
        match self {
            Self::Leaf { .. } => false,
            Self::Container { node, .. } => node.set_leaf_size(panel, size),
            Self::Split { children, .. } => {
                for child in children.iter_mut() {
                    if matches!(&child.node, Self::Leaf { panel: found } if *found == panel) {
                        if child.size == size {
                            return false;
                        }
                        child.size = size;
                        return true;
                    }
                    if child.node.set_leaf_size(panel, size) {
                        return true;
                    }
                }
                false
            }
        }
    }

    /// 在根横向条带的**最右边**追加一列（`column` 可以是叶子，也可以是容器）。
    ///
    /// 列宽策略：新列拿现有比例权重之和的一半 —— 即条带空间的 **1/3**，对齐
    /// niri 新窗口的默认列宽（1/3、1/2、2/3 三档里的 1/3）。已有列都乘同一个
    /// 因子（2/3）等比缩小，彼此的**相对**宽度一个像素都不变；帧宽本身不变，
    /// 所以叶子仍然精确铺满整屏。
    ///
    /// niri 的第一原则是"开新窗口不改变已有窗口大小"——那是在**无限长**的条带
    /// 上（视口跟着焦点滚动）。这里的条带就是固定大小的帧，没有滚动的视口，
    /// 只能在帧内按比例重新分配；等比缩小是"已有列宽只按比例重新分配"的那一
    /// 种，见 `frame.rs` 的文档。
    ///
    /// 根不是横向 split 时把整棵树包成横向：原树 2 份、新列 1 份（新列同样是
    /// 1/3）。`column` 里的面板已经出现在树上时返回 `false`，不重复加列。
    pub fn append_column(&mut self, column: Node) -> bool {
        let present = self.leaves();
        if column
            .leaves()
            .iter()
            .any(|panel| present.contains(panel))
        {
            return false;
        }
        match self {
            Self::Split {
                direction: Split::Horizontal,
                children,
                ..
            } => {
                let total: f32 = children
                    .iter()
                    .filter_map(|child| match child.size {
                        Size::Proportion(weight) => Some(weight.sanitized()),
                        Size::Fixed(_) => None,
                    })
                    .sum();
                let weight = if total > 0.0 { total / 2.0 } else { 1.0 };
                children.push(Child::new(Size::Proportion(weight), column));
                true
            }
            Self::Split { .. } | Self::Leaf { .. } | Self::Container { .. } => {
                let old = std::mem::replace(
                    self,
                    Self::Split {
                        direction: Split::Horizontal,
                        gap: NO_GAP,
                        children: Vec::new(),
                    },
                );
                if let Self::Split { children, .. } = self {
                    children.push(Child::proportion(2.0, old));
                    children.push(Child::proportion(1.0, column));
                }
                true
            }
        }
    }

    /// 把 `column` 追加到**包含 `near` 的那条横向 split** 的末尾（niri：新窗口往右加一列）。
    ///
    /// 找不到 `near` 时退回 [`Self::append_column`]。
    pub fn append_column_at(&mut self, near: PanelId, column: Node) -> bool {
        let present = self.leaves();
        if column.leaves().iter().any(|panel| present.contains(panel)) {
            return false;
        }
        if self.append_column_in(near, &column) {
            return true;
        }
        self.append_column(column)
    }

    /// 递归找包含 `near` 的横向 split，把 `column` 追加进去。
    fn append_column_in(&mut self, near: PanelId, column: &Node) -> bool {
        let here_has_near = self.leaves().contains(&near);
        let Self::Split {
            direction,
            children,
            ..
        } = self
        else {
            return false;
        };
        if *direction == Split::Horizontal && here_has_near {
            let total: f32 = children
                .iter()
                .filter_map(|child| match child.size {
                    Size::Proportion(weight) => Some(weight.sanitized()),
                    Size::Fixed(_) => None,
                })
                .sum();
            let weight = if total > 0.0 { total / 2.0 } else { 1.0 };
            children.push(Child::new(Size::Proportion(weight), column.clone()));
            return true;
        }
        let mut found = false;
        for child in children.iter_mut() {
            if child.node.leaves().contains(&near) {
                found = child.node.append_column_in(near, column);
                if found {
                    break;
                }
            }
        }
        found
    }

    /// 摘掉根条带里**包含 `panel` 的那一整列**（picker 关闭时用）。
    ///
    /// 只动根的直接子节点；其余子的尺寸权重原样保留，所以"加列 → 摘列"之后
    /// 树和加列前逐字节相同。根不是 split、或者没有这一列时返回 `false`。
    pub fn remove_column(&mut self, panel: PanelId) -> bool {
        // 列可能挂在某个嵌套的横向 split 里（主体那一行就是），所以先递归找
        // "直接子节点里含这个面板"的那一层，在该层摘掉；不要整块把主体行删了。
        let Self::Split { children, .. } = self else {
            return false;
        };
        if let Some(index) = children
            .iter()
            .position(|child| child.node.leaves().contains(&panel))
        {
            // 先看这个子节点自己能不能摘（它就是那一列）；
            // 摘不动说明列在它更深处，递归下去。
            let nested_removed = children[index].node.remove_column(panel);
            if !nested_removed {
                children.remove(index);
            }
            if let Some(only) = self.take_only_child() {
                *self = only;
            }
            return true;
        }
        // 根的直接子里没有它，但更深处可能有：递归每个子。
        for child in children.iter_mut() {
            if child.node.remove_column(panel) {
                return true;
            }
        }
        false
    }

    /// 递归摘掉一个叶子：装着它的那个子节点被删除，只剩一个子的 split 摊平。
    ///
    /// preview 关闭时用（列表那一列留下，preview 那一半消失、列表重新占满整列）。
    pub fn remove_leaf(&mut self, panel: PanelId) -> bool {
        let removed = match self {
            Self::Leaf { .. } => false,
            Self::Container { node, .. } => node.remove_leaf(panel),
            Self::Split { children, .. } => {
                let mut removed = false;
                for index in 0..children.len() {
                    if matches!(&children[index].node, Self::Leaf { panel: found } if *found == panel)
                    {
                        children.remove(index);
                        removed = true;
                        break;
                    }
                    if children[index].node.remove_leaf(panel) {
                        removed = true;
                        break;
                    }
                }
                removed
            }
        };
        if removed && let Some(only) = self.take_only_child() {
            *self = only;
        }
        removed
    }

    /// 把只剩一个子的 split 摊平成那个子（不归一化比例权重，几何不变）。
    fn take_only_child(&mut self) -> Option<Node> {
        let Self::Split { children, .. } = self else {
            return None;
        };
        if children.len() != 1 {
            return None;
        }
        children.pop().map(|child| child.node)
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
///    ├─ Complete   Fixed 0 格   ← 补全 tile：关着 0，打开 6 格
///    ├─ Input      Proportion 1  ← 提示行 + 键位提示行 + 面板装饰，吃掉右列剩余
///    └─ Status     Fixed 2 格   ← 细条里居中一行
/// ```
///
/// 数值理由：
/// * 转写是主区域，给 2 份；输入列 1 份，和 niri 的预设列宽 2/3 + 1/3 对齐。
/// * 输入是右列的比例子，剩余高度全归它；`28` 行的默认帧里就是 26 行
///   （原来的 `Fixed 4 格` 靠"第一个子吃剩余"得到同样的几何）。
/// * 状态 2 格：一行字在细条里居中，固定贴底。
/// * 补全 tile 初始 `Fixed(0.0)`：不占高度。打开时 [`crate::complete`] 把它改成
///   `Fixed(6 * ROW_HEIGHT)`，输入按比例自然缩短，总高度仍然精确等于右列高度。
///   这里**保留** 0 高度叶子（不摘除）：右列 gap = [`NO_GAP`]，0 高度不会多出缝；
///   而保留叶子让 `place_panels` 每帧都能把它写成 0 高度，重新打开时不用重建树。
/// * 树里 gap = [`NO_GAP`]：面板自己有边框和内边距，默认屏幕不需要额外缝，
///   叶子面积正好铺满整屏。
pub fn default_tree() -> Node {
    use crate::transcript::ROW_HEIGHT;

    // 条带 = 一根竖着的根：
    //   主体（转写 2/3 | 右列：留白 / 确认 / 补全 / 输入 4 行）
    //   键位提示行（横跨整条，1 行）
    //   状态行（横跨整条，2 行）
    //
    // 单位：`Size::Fixed` 是 **UI 像素**（`walk` 里乘一次 `PX_TO_METERS` 换成米），
    // 所以这里写 `ROW_HEIGHT * 行数`，**不要**再乘 `PX_TO_METERS`（乘了会变成 0.06px，
    // 固定子塌掉、"第一个子吃剩余"就会把同层位置全打乱）。
    Node::split(
        Split::Vertical,
        NO_GAP,
        vec![
            Child::proportion(1.0, main_row()),
            Child::fixed(1.0 * ROW_HEIGHT, Node::leaf(PanelId::Hints)),
            Child::fixed(2.0 * ROW_HEIGHT, Node::leaf(PanelId::Status)),
        ],
    )
}

/// 条带主体：左转写、右列（留白 / 确认 / 补全 / 输入）。
fn main_row() -> Node {
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
                        // 顶部留白：让输入区贴底。
                        Child::proportion(1.0, Node::leaf(PanelId::Spacer)),
                        // 确认行：没有待确认时 0 高度（`sync_confirm_tile` 按行数调）。
                        Child::fixed(0.0, Node::leaf(PanelId::Confirm)),
                        // 补全：关着时 0 高度（`sync_completion_tile` 调）。
                        Child::fixed(0.0, Node::leaf(PanelId::Complete)),
                        // 输入框初始 4 行；运行时由 `editor::sync_editor_tile`
                        // 按内容改成 `input_tile_height`（空输入保底 3 行、
                        // 最多 10 行 + 面板 chrome）。这里保留 Fixed：差额由
                        // 上面的 Spacer（比例子）吸收，右列总高精确不变。
                        Child::fixed(4.0 * ROW_HEIGHT, Node::leaf(PanelId::Input)),
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

    // 13. 默认树铺满整屏（叶子面积之和 = 区域面积），补全 tile 关着不占高度。
    #[test]
    fn the_default_tree_covers_the_screen() {
        let node = default_tree();
        let found = node.rects(area());
        assert_eq!(found.len(), 7, "转写 / 留白 / 确认 / 补全 / 输入 / 提示 / 状态");
        let mut ids: Vec<PanelId> = found.iter().map(|(id, _)| *id).collect();
        ids.sort_by_key(|id| format!("{id:?}"));
        assert_eq!(
            ids,
            vec![
                PanelId::Complete,
                PanelId::Confirm,
                PanelId::Hints,
                PanelId::Input,
                PanelId::Spacer,
                PanelId::Status,
                PanelId::Transcript
            ]
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
        // 左 2/3、右 1/3；右列从上到下是留白 / 补全 / 输入 / 状态。
        let transcript = found.iter().find(|(id, _)| *id == PanelId::Transcript).unwrap().1;
        let input = found.iter().find(|(id, _)| *id == PanelId::Input).unwrap().1;
        let status = found.iter().find(|(id, _)| *id == PanelId::Status).unwrap().1;
        let complete = found.iter().find(|(id, _)| *id == PanelId::Complete).unwrap().1;
        assert!(close(transcript.size.x, area().size.x * 2.0 / 3.0));
        assert!(close(status.size.y, 2.0 * crate::transcript::ROW_HEIGHT * PX_TO_METERS));
        assert!(close(bottom(&status), bottom(&area())));
        let hints = found.iter().find(|(id, _)| *id == PanelId::Hints).unwrap().1;
        assert!(close(bottom(&input), top(&hints)), "输入下方是提示行");
        assert!(close(bottom(&hints), top(&status)), "提示行下方是状态行");
        // 补全 tile 关着：高度 0，贴输入上方（右列底部是输入+状态，顶部是留白）。
        assert_eq!(complete.size.y, 0.0);
        assert!(close(top(&complete), top(&input)), "0 高度贴输入上沿");
        let spacer = found.iter().find(|(id, _)| *id == PanelId::Spacer).unwrap().1;
        assert!(close(top(&spacer), top(&area())), "留白贴顶");
        assert!(close(bottom(&spacer), top(&complete)), "留白一直铺到补全");
        assert_eq!(node.leaf_size(PanelId::Complete), Some(Size::Fixed(0.0)));
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

    // 16. 补全打开：tile 占 6 行，输入让出同样的高度，右列总高不变。
    #[test]
    fn opening_the_complete_tile_moves_the_input_up() {
        let mut node = default_tree();
        let closed = rects(&node);
        let input_closed = closed.iter().find(|(id, _)| *id == PanelId::Input).unwrap().1;
        let complete_closed = closed.iter().find(|(id, _)| *id == PanelId::Complete).unwrap().1;
        let spacer_closed = closed.iter().find(|(id, _)| *id == PanelId::Spacer).unwrap().1;
        assert_eq!(complete_closed.size.y, 0.0);

        let row = crate::transcript::ROW_HEIGHT;
        assert!(node.set_leaf_size(PanelId::Complete, Size::Fixed(6.0 * row)));
        assert!(!node.set_leaf_size(PanelId::Complete, Size::Fixed(6.0 * row)), "尺寸没变不写");

        let open = rects(&node);
        let complete = open.iter().find(|(id, _)| *id == PanelId::Complete).unwrap().1;
        let input = open.iter().find(|(id, _)| *id == PanelId::Input).unwrap().1;
        let status = open.iter().find(|(id, _)| *id == PanelId::Status).unwrap().1;
        let spacer = open.iter().find(|(id, _)| *id == PanelId::Spacer).unwrap().1;

        assert!(close(complete.size.y, 6.0 * row * PX_TO_METERS), "tile 正好 6 行");
        // 输入/状态固定高度且贴底：补全打开时它们**不动**，让位的是顶部留白。
        assert!(close(bottom(&input), bottom(&input_closed)), "输入底边不动");
        assert!(close(top(&input), top(&input_closed)), "输入整块不动");
        assert!(close(bottom(&complete), top(&input)), "补全紧贴输入上方");
        let hints = open.iter().find(|(id, _)| *id == PanelId::Hints).unwrap().1;
        assert!(close(bottom(&input), top(&hints)), "输入下方是提示行");
        assert!(close(bottom(&hints), top(&status)), "提示行下方是状态行");
        // 留白让出 6 行。
        assert!(
            close(spacer.size.y, spacer_closed.size.y - 6.0 * row * PX_TO_METERS),
            "留白让出 6 行：{} → {}",
            spacer_closed.size.y,
            spacer.size.y
        );
        assert!(close(top(&spacer), top(&area())), "留白仍然贴顶");
        let leaves: f32 = open.iter().map(|(_, rect)| rect.size.x * rect.size.y).sum();
        assert!(close(leaves, area().size.x * area().size.y), "叶子面积仍然铺满");
    }

    // 17. 尺寸读写：根就是叶子时没有尺寸可读，也改不动。
    #[test]
    fn a_root_leaf_has_no_size_to_read_or_write() {
        let mut node = leaf(PanelId::Transcript);
        assert_eq!(node.leaf_size(PanelId::Transcript), None);
        assert!(!node.set_leaf_size(PanelId::Transcript, Size::Fixed(18.0)));
        assert_eq!(node, leaf(PanelId::Transcript));
        // 不存在的叶子：读不到，也改不动。
        let mut tree = default_tree();
        assert_eq!(tree.leaf_size(PanelId::Preview), None);
        assert!(!tree.set_leaf_size(PanelId::Preview, Size::Fixed(18.0)));
    }

    /// 补全 tile 加入之前的 3 叶子树（转写 + 输入 + 状态），拆窗测试用。
    fn three_leaf_tree() -> Node {
        use crate::transcript::ROW_HEIGHT;
        Node::split(
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
                            Child::fixed(4.0 * ROW_HEIGHT, leaf(PanelId::Input)),
                            Child::fixed(2.0 * ROW_HEIGHT, leaf(PanelId::Status)),
                        ],
                    ),
                ),
            ],
        )
    }

    // 18. 拆窗：3 叶子 -> 4 叶子，两个转写并排，缝隙精确、面积仍然铺满。
    #[test]
    fn split_turns_the_transcript_into_two_side_by_side_panes() {
        let mut node = three_leaf_tree();
        assert_eq!(node.leaves().len(), 3, "初始 3 叶子");
        let before = rects(&node);
        let input_before = before.iter().find(|(id, _)| *id == PanelId::Input).unwrap().1;
        let status_before = before.iter().find(|(id, _)| *id == PanelId::Status).unwrap().1;
        let transcript_before = before.iter().find(|(id, _)| *id == PanelId::Transcript).unwrap().1;

        assert!(node.split_leaf(PanelId::Transcript, Split::Horizontal, PanelId::Pane2));
        assert_eq!(node.leaves().len(), 4, "拆完 4 叶子");

        let found = rects(&node);
        assert_eq!(found.len(), 4);
        let transcript = found.iter().find(|(id, _)| *id == PanelId::Transcript).unwrap().1;
        let pane2 = found.iter().find(|(id, _)| *id == PanelId::Pane2).unwrap().1;
        let input = found.iter().find(|(id, _)| *id == PanelId::Input).unwrap().1;
        let status = found.iter().find(|(id, _)| *id == PanelId::Status).unwrap().1;

        // 两个转写并排：同高、同一条水平线，中间恰好 DEFAULT_GAP。
        assert!(close(transcript.size.y, pane2.size.y));
        assert!(close(top(&transcript), top(&pane2)));
        assert!(close(bottom(&transcript), bottom(&pane2)));
        assert!(close(transcript.size.x, pane2.size.x), "两半等宽");
        assert!(close(
            left(&pane2) - right(&transcript),
            DEFAULT_GAP * PX_TO_METERS
        ));
        // 仍然占住原来的左列：左边界、右边界与拆前一致。
        assert!(close(left(&transcript), left(&transcript_before)));
        assert!(close(right(&pane2), right(&transcript_before)));
        // 右列两块完全没动。
        assert_eq!(input, input_before);
        assert_eq!(status, status_before);
        // 铺满：叶子面积 + 新加的缝 = 整屏面积。
        let leaves: f32 = found.iter().map(|(_, rect)| rect.size.x * rect.size.y).sum();
        let gap_area = DEFAULT_GAP * PX_TO_METERS * area().size.y;
        assert!(close(leaves + gap_area, area().size.x * area().size.y));
    }

    // 19. 已经拆过再拆：返回 false，树一个字节都不变。
    #[test]
    fn splitting_the_transcript_twice_is_a_no_op() {
        let mut node = three_leaf_tree();
        assert!(node.split_leaf(PanelId::Transcript, Split::Horizontal, PanelId::Pane2));
        let after_first = node.clone();
        assert!(!node.split_leaf(PanelId::Transcript, Split::Horizontal, PanelId::Pane2));
        assert_eq!(node, after_first, "重复拆不动树");
        assert_eq!(node.leaves().len(), 4);
    }

    // 20. 0 高度的兄弟不占地方，也不夺走比例子的剩余高度。
    #[test]
    fn a_zero_height_sibling_takes_no_room() {
        let node = Node::split(
            Split::Vertical,
            NO_GAP,
            vec![
                Child::fixed(0.0, leaf(PanelId::Complete)),
                Child::proportion(1.0, leaf(PanelId::Input)),
                Child::fixed(2.0 * crate::transcript::ROW_HEIGHT, leaf(PanelId::Status)),
            ],
        );
        let found = rects(&node);
        let complete = found.iter().find(|(id, _)| *id == PanelId::Complete).unwrap().1;
        let input = found.iter().find(|(id, _)| *id == PanelId::Input).unwrap().1;
        let status = found.iter().find(|(id, _)| *id == PanelId::Status).unwrap().1;
        assert_eq!(complete.size.y, 0.0);
        assert!(close(top(&complete), top(&area())));
        assert!(close(top(&input), top(&complete)), "输入从 tile 底部开始");
        assert!(
            close(
                input.size.y,
                area().size.y - 2.0 * crate::transcript::ROW_HEIGHT * PX_TO_METERS
            ),
            "比例子拿到除固定行外的全部高度"
        );
        assert!(close(bottom(&input), top(&status)));
        assert!(close(bottom(&status), bottom(&area())));
    }

    // 21. 条带最右边加一列：新列 1/3，已有列等比缩到 2/3（相对宽度不变）。
    #[test]
    fn appending_a_column_takes_a_third_and_scales_the_old_columns() {
        let mut node = default_tree();
        let before = rects(&node);
        assert!(node.append_column(Node::container(
            PanelId::Picker,
            Node::leaf(PanelId::List)
        )));
        let after = rects(&node);

        assert_eq!(after.len(), before.len() + 1, "只多一个叶子");
        assert_eq!(after.last().map(|(id, _)| *id), Some(PanelId::List));
        let column = after.last().expect("新列").1;
        assert!(close(column.size.x, area().size.x / 3.0), "新列 = 条带的 1/3");
        assert!(close(right(&column), right(&area())), "新列贴住最右");
        assert_eq!(node.leaves().last(), Some(&PanelId::List));

        for (id, old) in &before {
            let fresh = after
                .iter()
                .find(|(found, _)| found == id)
                .map(|(_, rect)| *rect)
                .unwrap_or_else(|| panic!("{id:?} 还在树上"));
            assert!(
                close(fresh.size.x, old.size.x * 2.0 / 3.0),
                "{id:?} 按比例缩到 2/3"
            );
            assert!(close(fresh.size.y, old.size.y));
            assert!(close(top(&fresh), top(old)));
            if *id == PanelId::Transcript {
                assert!(close(left(&fresh), left(old)), "最左列左边界不动");
            }
        }
        // 叶子面积仍然铺满整屏。
        let leaves: f32 = after.iter().map(|(_, rect)| rect.size.x * rect.size.y).sum();
        assert!(close(leaves, area().size.x * area().size.y));
    }

    // 22. 加列 → 摘列：树逐字节复原；重复摘是空操作。
    #[test]
    fn removing_the_appended_column_restores_the_tree() {
        let before = default_tree();
        let mut node = before.clone();
        assert!(node.append_column(Node::container(
            PanelId::Picker,
            Node::leaf(PanelId::List)
        )));
        assert!(node.remove_column(PanelId::List));
        assert_eq!(node, before, "加列再摘列不改树的其它部分");
        assert!(!node.remove_column(PanelId::List), "没有这一列时返回 false");
        assert_eq!(node, before);
    }

    // 23. 容器不是叶子：rects 只给叶子，containers 单独给容器区域。
    #[test]
    fn containers_are_not_leaves_but_have_their_own_area() {
        let node = Node::split(
            Split::Horizontal,
            NO_GAP,
            vec![
                Child::proportion(2.0, leaf(PanelId::Transcript)),
                Child::new(
                    Size::Proportion(1.0),
                    Node::container(PanelId::Picker, Node::leaf(PanelId::List)),
                ),
            ],
        );
        assert_eq!(node.leaves(), vec![PanelId::Transcript, PanelId::List]);
        let all = node.placements(area());
        assert_eq!(all.len(), 3, "转写 + 容器 + 列表");
        assert_eq!(all[1].0, PanelId::Picker);
        assert!(all[1].2, "第二项是容器");
        assert_eq!(all[2].0, PanelId::List);
        assert!(!all[2].2);

        let leaf_rects = node.rects(area());
        assert_eq!(leaf_rects.len(), 2);
        let containers = node.containers(area());
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].0, PanelId::Picker);
        assert_eq!(containers[0].1, all[2].1, "容器和内部列表区域相同");
    }

    // 24. 自定义尺寸的拆叶子：2:1 竖直拆分；摘掉 preview 后树精确复原。
    #[test]
    fn splitting_a_leaf_with_custom_sizes_can_be_undone() {
        let before = three_leaf_tree();
        let mut node = before.clone();
        let input_before = rects(&node)
            .into_iter()
            .find(|(id, _)| *id == PanelId::Input)
            .expect("输入")
            .1;
        assert!(node.split_leaf_sized(
            PanelId::Input,
            Split::Vertical,
            PanelId::Preview,
            NO_GAP,
            Size::Proportion(2.0),
            Size::Proportion(1.0),
        ));
        let found = rects(&node);
        let input = found.iter().find(|(id, _)| *id == PanelId::Input).unwrap().1;
        let preview = found.iter().find(|(id, _)| *id == PanelId::Preview).unwrap().1;
        assert!(close(input.size.x, preview.size.x), "同一列同宽");
        assert!(close(input.size.y, preview.size.y * 2.0), "2:1");
        assert!(close(input.size.y + preview.size.y, input_before.size.y));
        assert!(close(bottom(&input), top(&preview)), "没有缝");
        // 右列从下到上是 状态 / preview / 输入：preview 的底边 = 状态行顶边。
        let status = found
            .iter()
            .find(|(id, _)| *id == PanelId::Status)
            .expect("状态行")
            .1;
        assert!(close(top(&input), top(&area())), "输入贴右列顶部");
        assert!(close(bottom(&preview), top(&status)), "preview 贴着状态行");

        assert!(node.remove_leaf(PanelId::Preview));
        assert_eq!(node, before, "摘掉 preview 后精确复原");
        assert!(!node.remove_leaf(PanelId::Preview));
    }
}
