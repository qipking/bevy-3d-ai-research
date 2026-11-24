//! 相机操作架：手柄/键盘围绕一个目标点转 + 前后移动。
//!
//! 桌面开发期用**手柄**模拟移动端的"移动 + 转向"（左摇杆平移目标点，右摇杆 yaw/pitch，
//! 扳机/肩键拉近拉远）；没有手柄时用键盘 WASD + 方向键兜底。
//! 移动端接入陀螺仪时，只需要把 `yaw/pitch` 换成设备姿态（其余逻辑不变）。
//!
//! 默认关闭（`RF_RIG=1` 打开），以免打乱自动巡游的固定机位。

use bevy::input::gamepad::{Gamepad, GamepadAxis, GamepadButton};
use bevy::prelude::*;

/// 相机实体标记（前端和后端都用它找到唯一的 3D 相机）。
#[derive(Component)]
pub struct RigCamera;

/// 相机绕目标点的球坐标。
#[derive(Resource)]
pub struct CameraRig {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub enabled: bool,
}

impl Default for CameraRig {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.06,
            distance: 1.45,
            enabled: std::env::var("RF_RIG").is_ok(),
        }
    }
}

pub struct CameraRigPlugin;

impl Plugin for CameraRigPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraRig>()
            .add_systems(Update, drive_rig);
    }
}

/// 手柄轴值（多个手柄相加），没有手柄就是 0。
fn gamepad_axes(gamepads: &Query<&Gamepad>) -> (Vec2, Vec2, f32) {
    let mut left = Vec2::ZERO;
    let mut right = Vec2::ZERO;
    let mut zoom = 0.0;
    for gamepad in gamepads.iter() {
        left += Vec2::new(
            gamepad.get(GamepadAxis::LeftStickX).unwrap_or(0.0),
            gamepad.get(GamepadAxis::LeftStickY).unwrap_or(0.0),
        );
        right += Vec2::new(
            gamepad.get(GamepadAxis::RightStickX).unwrap_or(0.0),
            gamepad.get(GamepadAxis::RightStickY).unwrap_or(0.0),
        );
        // 扳机在 Bevy 里是**按钮**（GamepadButton::LeftTrigger2/RightTrigger2），不是轴
        let l = gamepad.get(GamepadButton::LeftTrigger2).unwrap_or(0.0);
        let r = gamepad.get(GamepadButton::RightTrigger2).unwrap_or(0.0);
        zoom += l - r;
    }
    (left, right, zoom)
}

fn drive_rig(
    time: Res<Time>,
    mut rig: ResMut<CameraRig>,
    gamepads: Query<&Gamepad>,
    keys: Res<ButtonInput<KeyCode>>,
    mut camera: Query<&mut Transform, With<RigCamera>>,
) {
    if !rig.enabled {
        return;
    }
    let dt = time.delta_secs();
    let (left, right, zoom) = gamepad_axes(&gamepads);

    // 手柄：右摇杆转向，左摇杆平移，扳机缩放
    rig.yaw -= right.x * 1.8 * dt;
    rig.pitch = (rig.pitch + right.y * 1.2 * dt).clamp(-0.6, 0.6);
    let speed = rig.distance * 0.9;
    rig.target += Vec3::new(left.x, -left.y, 0.0) * speed * dt;
    rig.distance = (rig.distance * (1.0 - zoom * 1.5 * dt)).clamp(0.35, 6.0);

    // 键盘兜底：A/D 平移，W/S 前后，方向键转向，Q/E 缩放
    let mut move_dir = Vec2::ZERO;
    if keys.pressed(KeyCode::KeyA) {
        move_dir.x -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) {
        move_dir.x += 1.0;
    }
    if keys.pressed(KeyCode::KeyW) {
        move_dir.y += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) {
        move_dir.y -= 1.0;
    }
    rig.target += Vec3::new(move_dir.x, -move_dir.y, 0.0) * speed * dt;
    if keys.pressed(KeyCode::ArrowLeft) {
        rig.yaw += 1.8 * dt;
    }
    if keys.pressed(KeyCode::ArrowRight) {
        rig.yaw -= 1.8 * dt;
    }
    if keys.pressed(KeyCode::ArrowUp) {
        rig.pitch = (rig.pitch + 1.2 * dt).clamp(-0.6, 0.6);
    }
    if keys.pressed(KeyCode::ArrowDown) {
        rig.pitch = (rig.pitch - 1.2 * dt).clamp(-0.6, 0.6);
    }
    if keys.pressed(KeyCode::KeyQ) {
        rig.distance = (rig.distance * (1.0 + 1.2 * dt)).clamp(0.35, 6.0);
    }
    if keys.pressed(KeyCode::KeyE) {
        rig.distance = (rig.distance * (1.0 - 1.2 * dt)).clamp(0.35, 6.0);
    }

    let rotation = Quat::from_rotation_y(rig.yaw) * Quat::from_rotation_x(-rig.pitch);
    let position = rig.target + rotation * Vec3::new(0.0, 0.0, rig.distance);
    if let Ok(mut transform) = camera.single_mut() {
        *transform = Transform::from_translation(position).looking_at(rig.target, Vec3::Y);
    }
}
