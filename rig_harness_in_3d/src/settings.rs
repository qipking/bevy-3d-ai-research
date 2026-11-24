//! `[view]`：本 app 的用户设置（`RIG_HOME/settings.toml`）。
//!
//! 路径规则用上游自己的 [`Home::settings`]：设置只来自 `RIG_HOME`，项目目录里
//! 没有 rig 设置（上游明确说 "A project has no settings"）。读取仍走
//! [`AppSettingsExt::add_setting`]；文件读不了、不是 TOML、或 `[view]` 表读不成
//! [`ViewSettings`] 时，插默认值并 `warn!`，不 panic、也不让上游 launcher 的
//! `Settings::problems` 拒绝启动（那会直接结束进程，不是降级）。
//!
//! ```toml
//! [view]
//! hide_status = false
//! ```

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use rig_ecs::prelude::Notice;
use rig_harness::commands::CommandArgs;
use rig_harness::prelude::{AppSettingsExt, RigHome, Settings};
use serde::Deserialize;

/// `settings.toml` 的真实路径；保存设置时写它。
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct SettingsPath(pub PathBuf);

/// `[view]` 表。
#[derive(Resource, Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ViewSettings {
    /// 不画状态行（`tui.hide_status` 的等价物）。
    pub hide_status: bool,
}

/// 本 app 读的设置段名。
pub const SECTION: &str = "view";

/// 启动时读 `path` 进 `Settings` 资源；文件不存在是空设置，坏文件留给 [`register`] 降级。
pub fn install(app: &mut App, path: PathBuf) {
    app.insert_resource(SettingsPath(path.clone()))
        .insert_resource(Settings::read(&path));
}

/// 在插件 `build` 里读 `[view]`。
///
/// 两步降级：
/// 1. 文件级问题（读不了 / 不是 TOML / `[plugins]` 坏了）：`warn!` 后用空设置，
///    这样 launcher 的 `finish` 不会因为 `problems` 拒绝运行；
/// 2. 表级问题（`[view]` 里有未知键 / 类型不对）：`add_setting` 自己会 `warn!`
///    并记 problem；这里再清掉 problem 并说明整段用了默认值。
pub fn register(app: &mut App) {
    if !app.world().contains_resource::<SettingsPath>() {
        let path = RigHome::of(app.world()).settings();
        app.insert_resource(SettingsPath(path));
    }
    if !app.world().contains_resource::<Settings>() {
        let path = app.world().resource::<SettingsPath>().0.clone();
        app.insert_resource(Settings::read(&path));
    }
    let path = app.world().resource::<SettingsPath>().0.clone();

    let file_problems = app.world().resource::<Settings>().problems().to_vec();
    if !file_problems.is_empty() {
        for problem in &file_problems {
            warn!("[settings] {problem}");
        }
        warn!(
            "[settings] {}: 文件不可读或不是 TOML；使用默认值",
            path.display()
        );
        app.insert_resource(Settings::default());
    }

    // 上游的读法：`[view]` 表 -> `ViewSettings`；键错/类型错会 warn 并记 problem。
    app.add_setting::<ViewSettings>(SECTION);

    let table_problems = app.world().resource::<Settings>().problems().len();
    if table_problems > 0 {
        warn!(
            "[settings] [{}] 读取失败；使用默认值（具体键见上面的 warn）",
            SECTION
        );
        // 同样是降级而不是拒绝启动。
        app.insert_resource(Settings::default());
    }

    let hide = app.world().resource::<ViewSettings>().hide_status;
    if path.is_file() && file_problems.is_empty() && table_problems == 0 {
        info!("[settings] {} (view.hide_status = {hide})", path.display());
    } else {
        info!(
            "[settings] {} (不存在或已降级；view.hide_status = {hide})",
            path.display()
        );
    }
}

/// `/setting` 命令：`/setting` 显示当前值，`/setting hide_status <true|false>` 改并保存。
pub fn setting(
    In(args): In<CommandArgs>,
    mut view: ResMut<ViewSettings>,
    path: Res<SettingsPath>,
    mut notices: MessageWriter<Notice>,
) {
    let parts: Vec<&str> = args.args.split_whitespace().collect();
    match parts.as_slice() {
        [] => {
            notices.write(Notice::info(
                args.agent,
                format!(
                    "view.hide_status = {}（文件：{}）",
                    view.hide_status,
                    path.0.display()
                ),
            ));
        }
        [name, value] => {
            let name = name.strip_prefix("view.").unwrap_or(name);
            if name != "hide_status" {
                notices.write(Notice::error(
                    args.agent,
                    format!("没有设置 `{name}`；可用：view.hide_status <true|false>"),
                ));
                return;
            }
            let Some(value) = parse_bool(value) else {
                notices.write(Notice::error(
                    args.agent,
                    format!("`{value}` 不是布尔值；用法：/setting hide_status <true|false>"),
                ));
                return;
            };
            view.hide_status = value;
            match save(&path.0, &view) {
                Ok(()) => {
                    notices.write(Notice::info(
                        args.agent,
                        format!(
                            "view.hide_status = {value}（已保存到 {}）",
                            path.0.display()
                        ),
                    ));
                }
                Err(failure) => {
                    notices.write(Notice::warn(
                        args.agent,
                        format!(
                            "view.hide_status = {value}（只改在内存；写 {} 失败：{failure}）",
                            path.0.display()
                        ),
                    ));
                }
            }
        }
        _ => {
            notices.write(Notice::error(
                args.agent,
                "用法：/setting hide_status <true|false>",
            ));
        }
    }
}

/// `true` / `false`，也接受 `on` / `off` / `1` / `0`。
fn parse_bool(text: &str) -> Option<bool> {
    match text.to_ascii_lowercase().as_str() {
        "true" | "on" | "1" => Some(true),
        "false" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// 把 `[view]` 写回 `path`，文件里其他表原样保留。
///
/// 不引 `toml` 依赖（本 crate 是模板，`Cargo.toml` 不归这一轮改）：只做
/// 段级文本替换——找到 `[view]`（及其子表）的区间换掉，没有就追加。注释和
/// 其他段不动。
pub fn save(path: &Path, view: &ViewSettings) -> io::Result<()> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => String::new(),
        Err(failure) => return Err(failure),
    };
    let block = format!("[{SECTION}]\nhide_status = {}\n", view.hide_status);
    fs::write(path, replace_section(&text, SECTION, &block))
}

/// 一段表头行里的表名（支持 `[view] # 注释`；`[[array]]` 不是本 app 的段）。
fn table_name(line: &str) -> Option<&str> {
    let rest = line.trim().strip_prefix('[')?;
    let end = rest.find(']')?;
    Some(rest[..end].trim())
}

fn replace_section(text: &str, section: &str, block: &str) -> String {
    let mut out = String::with_capacity(text.len() + block.len());
    let mut in_target = false;
    let mut wrote = false;
    for line in text.lines() {
        if let Some(name) = table_name(line) {
            let target = name == section || name.starts_with(&format!("{section}."));
            if target {
                if !wrote {
                    out.push_str(block);
                    wrote = true;
                }
                in_target = true;
                continue;
            }
            in_target = false;
        }
        if !in_target {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !wrote {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(block);
    }
    out
}
