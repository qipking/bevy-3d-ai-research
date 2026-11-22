//! 斗兽场（漏斗）空间布局：一层一条轨道 = 一个 agent，卡片是卫星，永远朝相机。
//!
//! 几何（见 `../docs/design/SPATIAL_UI.md` §13/§14）：
//!
//! ```text
//! 层 L：R_L = R0 + L·ΔR,  Y_L = Y0 + L·ΔY
//! 座位 k：θ = k·Δθ,  P = (R_L·sinθ, Y_L, −R_L·cosθ)
//! 卡宽  ：2·R_L·sin(Δθ/2) − gap          （外层更大 → 更宽）
//! 卡高  ：顶边固定在 Y_L + h0/2，展开时**向下**长（下面的层已隐藏，不会撞人）
//! 朝向  ：每帧朝相机（billboard），所以同层卡片互不遮挡、永远正对阅读
//! ```
//!
//! 这不是"把 UI 摆成 3D"，是 `docs/design/SPATIAL_UI.md` 定的模型：**恒星（相机）在中心，
//! 行星（agent）占轨道层，卫星（卡片）沿轨道排开**。

use bevy::prelude::*;
use bevy_ui::{BorderRadius, Node, UiRect, px};
use feathers_in_3d::{MirrorRoot, WorldLayer, WorldPanel};

use crate::camera_rig::RigCamera;
use crate::frame::UNITS_TO_WORLD;
use crate::transcript::{Row, TranscriptFont, spawn_row};

/// 漏斗的几何参数（初值来自 `docs/design/SPATIAL_UI.md` §13.2，可调）。
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct Funnel {
    /// 第 0 层的轨道半径（米）。
    pub radius0: f32,
    /// 每往外一层半径加多少。
    pub radius_step: f32,
    /// 第 0 层的高度（米）。
    pub height0: f32,
    /// 每往外一层相机/轨道抬高多少。
    pub height_step: f32,
    /// 一个座位占的圆心角（弧度）。
    pub arc: f32,
    /// 同一层相邻卡片之间的缝（米）。
    pub gap: f32,
    /// 收起状态的卡片高度（米）：顶边在 `Y_L + height0/2`，向下展开。
    pub card_height: f32,
}

impl Default for Funnel {
    fn default() -> Self {
        Self {
            radius0: 1.0,
            radius_step: 0.20,
            height0: 0.0,
            height_step: 0.30,
            arc: 22f32.to_radians(),
            gap: 0.03,
            card_height: 0.32,
        }
    }
}

impl Funnel {
    /// 第 `layer` 层的轨道半径。
    pub fn radius(&self, layer: u16) -> f32 {
        self.radius0 + f32::from(layer) * self.radius_step
    }

    /// 第 `layer` 层的高度（= 相机停在这一层时的高度）。
    pub fn height(&self, layer: u16) -> f32 {
        self.height0 + f32::from(layer) * self.height_step
    }

    /// 第 `slot` 个座位的角（弧度）。
    pub fn angle(&self, slot: i16) -> f32 {
        f32::from(slot) * self.arc
    }

    /// 该层卡片的宽度（米）：切向弦长减缝，所以外层天然更宽。
    pub fn card_width(&self, layer: u16) -> f32 {
        2.0 * self.radius(layer) * (self.arc * 0.5).sin() - self.gap
    }

    /// 一圈能放几个座位。
    pub fn seats_per_ring(&self) -> u16 {
        (std::f32::consts::TAU / self.arc).floor() as u16
    }

    /// 座位的世界坐标（未朝相机）。
    pub fn seat(&self, layer: u16, slot: i16) -> Vec3 {
        let angle = self.angle(slot);
        let radius = self.radius(layer);
        Vec3::new(radius * angle.sin(), self.height(layer), -radius * angle.cos())
    }

    /// 相机停在某一层时的眼睛高度：卡片坐在座位上（底边 = `Y_L`），眼睛在卡片中间。
    pub fn eye_height(&self, layer: u16) -> f32 {
        self.height(layer) + self.card_height * 0.5
    }

    /// 卡片中心：底边坐在 `Y_L`；**展开时长出来的部分向下垂**（下面的层已隐藏）。
    pub fn card_center(&self, layer: u16, slot: i16, height: f32) -> Vec3 {
        let mut center = self.seat(layer, slot);
        center.y += self.card_height - height * 0.5;
        center
    }
}

/// 朝相机的旋转：+Z 指向相机、+Y 朝上（文字方向正常，不镜像）。
pub fn billboard(position: Vec3, camera: Vec3) -> Quat {
    let z = (camera - position).normalize_or_zero();
    if z == Vec3::ZERO {
        return Quat::IDENTITY;
    }
    let x = Vec3::Y.cross(z);
    if x.length_squared() < 1e-8 {
        // 相机正在卡片正上/正下方：用 Z 轴做右向，避免退化。
        return Quat::from_mat3(&Mat3::from_cols(Vec3::Z, Vec3::Y, Vec3::X));
    }
    let x = x.normalize();
    let y = z.cross(x);
    Quat::from_mat3(&Mat3::from_cols(x, y, z))
}

/// 一颗行星 = 一条轨道层。卡片挂在这个层上。
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Card {
    /// 第几层（= 哪个 agent / 行星）。
    pub layer: u16,
    /// 这一层里的第几个座位。
    pub slot: i16,
    /// 卡片高度（米）。展开的长卡就靠它变高、向下长。
    pub height: f32,
}

/// 当前聚焦的层（相机高度）。更近恒星的层（`< layer`）隐藏。
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpatialFocus {
    pub layer: u16,
}

/// 每帧给每张卡摆位 + 朝相机；聚焦层以下的层收起来。
pub fn place_cards(
    funnel: Res<Funnel>,
    focus: Res<SpatialFocus>,
    camera: Query<&GlobalTransform, With<RigCamera>>,
    mut cards: Query<(&Card, &mut WorldPanel)>,
) {
    let Ok(camera) = camera.single() else {
        return;
    };
    let eye = camera.translation();
    for (card, mut panel) in cards.iter_mut() {
        let mut position = funnel.card_center(card.layer, card.slot, card.height);
        // 更近恒星的下层：藏到视野外（P2 会换成真正的 hidden，交给镜像层处理）。
        if card.layer < focus.layer {
            position.y -= 100.0;
        }
        let rotation = billboard(position, eye);
        let anchor = Transform {
            translation: position,
            rotation,
            scale: Vec3::ONE,
        };
        if panel.anchor != anchor {
            panel.anchor = anchor;
        }
    }
}

/// 一张卡片的内容：标题行 + 若干行。
pub struct CardContent<'a> {
    pub title: &'a str,
    pub rows: Vec<Row>,
}

/// 观感：卡片底色、边、圆角。
#[derive(Clone, Copy, Debug)]
pub struct CardLook {
    pub background: Color,
    pub border: Color,
    pub radius: f32,
}

impl Default for CardLook {
    fn default() -> Self {
        Self {
            background: Color::srgb(0.055, 0.065, 0.09),
            border: Color::srgb(0.28, 0.45, 0.7),
            radius: 10.0,
        }
    }
}

/// 生成一张卡片：`bevy_ui` 子树（尺寸按漏斗算）+ 3D 面板 + 行。
pub fn spawn_card(
    commands: &mut Commands,
    funnel: &Funnel,
    font: &TranscriptFont,
    card: Card,
    look: CardLook,
    content: CardContent,
) -> Entity {
    let size = Vec2::new(funnel.card_width(card.layer), card.height);
    let entity = spawn_card_sized(commands, font, size, look, content);
    commands.entity(entity).insert(card);
    entity
}

/// 生成一张给定尺寸（米）的卡片：平铺布局自己算尺寸时用这个。
pub fn spawn_card_sized(
    commands: &mut Commands,
    font: &TranscriptFont,
    size: Vec2,
    look: CardLook,
    content: CardContent,
) -> Entity {
    let width_px = size.x / UNITS_TO_WORLD;
    let height_px = size.y / UNITS_TO_WORLD;
    let entity = commands
        .spawn((
            Name::new(format!("card:{:.2}x{:.2}", size.x, size.y)),
            Node {
                position_type: PositionType::Absolute,
                // 排在屏幕外：3D 副本才是可见的那个。
                left: px(-20000.0),
                top: px(0.0),
                width: px(width_px),
                height: px(height_px),
                padding: UiRect::all(px(8.0)),
                border: UiRect::all(px(2.0)),
                border_radius: BorderRadius::all(px(look.radius)),
                flex_direction: FlexDirection::Column,
                row_gap: px(0.0),
                ..default()
            },
            BackgroundColor(look.background),
            BorderColor::all(look.border),
            WorldPanel {
                units_to_world: UNITS_TO_WORLD,
                anchor: Transform::default(),
            },
            WorldLayer(0),
            MirrorRoot,
        ))
        .id();
    let title = content.title.to_string();
    commands.entity(entity).with_children(|parent| {
        // 标题一行（用行系统画，保证和正文同字体/同裁剪）。
        let title_row = Row::new(rig_tui::ratatui::text::Line::from(
            rig_tui::ratatui::text::Span::styled(
                title,
                rig_tui::ratatui::style::Style::default()
                    .fg(rig_tui::ratatui::style::Color::LightYellow)
                    .add_modifier(rig_tui::ratatui::style::Modifier::BOLD),
            ),
        ));
        spawn_row(parent, &title_row, &font.0);
        for row in &content.rows {
            spawn_row(parent, row, &font.0);
        }
    });
    entity
}

/// 总览里的自由相机：WASD 平移、Q/E 升降、方向键或鼠标右键拖动转向。
///
/// 用户的要求："总览的时候可以在圆柱体内部自由移动相机位置和视角"。相机就是普通的
/// `RigCamera`，这个系统只在 [`FreeLook`] 打开时动它，不影响巡游/演示里的固定机位。
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FreeLook(pub bool);

/// 自由相机的速度和转向速度。
#[derive(Resource, Clone, Copy, Debug)]
pub struct FreeLookSpeed {
    pub move_per_second: f32,
    pub turn_per_second: f32,
}

impl Default for FreeLookSpeed {
    fn default() -> Self {
        Self {
            move_per_second: 0.9,
            turn_per_second: 1.4,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn free_look(
    time: Res<Time>,
    look: Res<FreeLook>,
    speed: Res<FreeLookSpeed>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<bevy::input::mouse::MouseButton>>,
    mut motion: MessageReader<bevy::input::mouse::MouseMotion>,
    mut camera: Query<&mut Transform, With<RigCamera>>,
) {
    if !look.0 {
        motion.clear();
        return;
    }
    let Ok(mut transform) = camera.single_mut() else {
        return;
    };
    let dt = time.delta_secs();
    // 鼠标右键拖动转向（没有鼠标时用方向键）。
    let mut turn = Vec2::ZERO;
    if buttons.pressed(bevy::input::mouse::MouseButton::Right) {
        for event in motion.read() {
            turn += event.delta;
        }
    } else {
        motion.clear();
    }
    if keys.pressed(KeyCode::ArrowLeft) {
        turn.x -= speed.turn_per_second * 100.0 * dt;
    }
    if keys.pressed(KeyCode::ArrowRight) {
        turn.x += speed.turn_per_second * 100.0 * dt;
    }
    if keys.pressed(KeyCode::ArrowUp) {
        turn.y -= speed.turn_per_second * 100.0 * dt;
    }
    if keys.pressed(KeyCode::ArrowDown) {
        turn.y += speed.turn_per_second * 100.0 * dt;
    }
    if turn != Vec2::ZERO {
        let yaw = Quat::from_rotation_y(-turn.x * 0.002);
        let pitch = Quat::from_rotation_x(-turn.y * 0.002);
        transform.rotation = yaw * transform.rotation * pitch;
    }
    // WASD 平移（沿相机自己的轴），Q/E 升降。
    let mut move_dir = Vec3::ZERO;
    if keys.pressed(KeyCode::KeyW) {
        move_dir += *transform.forward();
    }
    if keys.pressed(KeyCode::KeyS) {
        move_dir += *transform.back();
    }
    if keys.pressed(KeyCode::KeyA) {
        move_dir += *transform.left();
    }
    if keys.pressed(KeyCode::KeyD) {
        move_dir += *transform.right();
    }
    if keys.pressed(KeyCode::KeyQ) {
        move_dir += Vec3::NEG_Y;
    }
    if keys.pressed(KeyCode::KeyE) {
        move_dir += Vec3::Y;
    }
    if move_dir != Vec3::ZERO {
        transform.translation += move_dir.normalize() * speed.move_per_second * dt;
    }
}

/// 空间布局插件：资源 + 每帧摆位 + 总览自由相机。
pub struct SpatialPlugin;

impl Plugin for SpatialPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Funnel>()
            .init_resource::<SpatialFocus>()
            .init_resource::<FreeLook>()
            .init_resource::<FreeLookSpeed>()
            .add_systems(Update, (place_cards, free_look));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_grow_outward_and_upward() {
        let funnel = Funnel::default();
        assert_eq!(funnel.radius(0), 1.0);
        assert!((funnel.radius(2) - 1.4).abs() < 1e-6);
        assert_eq!(funnel.height(0), 0.0);
        assert!((funnel.height(2) - 0.6).abs() < 1e-6);
        // 外层更宽（斗兽场第二排座位）
        assert!(funnel.card_width(2) > funnel.card_width(1));
        assert!(funnel.card_width(1) > funnel.card_width(0));
    }

    #[test]
    fn a_card_sits_on_its_seat() {
        let funnel = Funnel::default();
        let collapsed = funnel.card_center(0, 0, funnel.card_height);
        // 底边 = 座位高度（Y_L），所以卡片是"坐在座位上的"。
        let bottom = collapsed.y - funnel.card_height * 0.5;
        assert!((bottom - funnel.height(0)).abs() < 1e-6);
        // 眼睛在卡片中间。
        assert!((funnel.eye_height(0) - collapsed.y).abs() < 1e-6);
    }

    #[test]
    fn a_ring_holds_seats_around_it() {
        let funnel = Funnel::default();
        assert_eq!(funnel.seats_per_ring(), 16); // 2π / 22° = 16.36
        // 第 0 个座位在正前方（-Z），第 1 个在右边。
        let front = funnel.seat(0, 0);
        assert!(front.x.abs() < 1e-6 && (front.z + 1.0).abs() < 1e-6);
        let right = funnel.seat(0, 1);
        assert!(right.x > 0.0 && right.z < 0.0);
        // 所有座位都在该层半径上、同一高度。
        for slot in -(i16::try_from(funnel.seats_per_ring() / 2).unwrap())..=i16::try_from(funnel.seats_per_ring() / 2).unwrap() {
            let seat = funnel.seat(3, slot);
            assert!((Vec2::new(seat.x, seat.z).length() - funnel.radius(3)).abs() < 1e-5);
            assert!((seat.y - funnel.height(3)).abs() < 1e-6);
        }
    }

    #[test]
    fn expanding_a_card_grows_it_downward() {
        let funnel = Funnel::default();
        let collapsed = funnel.card_center(1, 0, funnel.card_height);
        let expanded = funnel.card_center(1, 0, funnel.card_height + 0.5);
        // 顶边不动：中心下移半个增量。
        assert!((collapsed.y - expanded.y - 0.25).abs() < 1e-6);
        let top = |center: Vec3, height: f32| center.y + height * 0.5;
        assert!((top(collapsed, funnel.card_height) - top(expanded, funnel.card_height + 0.5)).abs() < 1e-6);
    }

    #[test]
    fn billboard_faces_the_camera() {
        let position = Vec3::new(0.0, 0.0, -1.0);
        let camera = Vec3::new(0.2, 0.1, 0.5);
        let rotation = billboard(position, camera);
        // 卡片的 +Z 应该指向相机。
        let forward = rotation * Vec3::Z;
        let to_camera = (camera - position).normalize();
        assert!(forward.dot(to_camera) > 0.999);
        // +Y 仍然大致朝上（没有翻滚）。
        let up = rotation * Vec3::Y;
        assert!(up.y > 0.99);
    }
}
