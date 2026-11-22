//! Painter-order for panel nodes, the way bevy_luma does it.
//!
//! A world-space UI panel is a stack of coplanar quads. Their geometry cannot say
//! which one wins, so the order has to come from the render sort key:
//!   `TransparentSortingInfo3d::Sorted { mesh_center, depth_bias }`
//! with the *same* `mesh_center` for every node of a panel (the panel's centre) and a
//! `depth_bias` that grows with UI stacking. `Transparent3d` sorts the resulting
//! `distance + depth_bias` ascending (far → near), so a larger bias draws later, i.e.
//! on top — exactly the UI painter's order, at any viewing angle.
//!
//! Reference: `bevy_luma/crates/luma_engine/src/layout/render/depth_sync.rs`
//! (`world3d_layer_sort_bias` + `apply_world3d_ui_transparent_sort_overrides`).

use bevy::core_pipeline::core_3d::{Transparent3d, TransparentSortingInfo3d};
use bevy::prelude::*;
use bevy::render::render_phase::{PhaseItem, ViewSortedRenderPhases, sort_phase_system};
use bevy::render::sync_world::MainEntity;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};

/// How much bias one painter step adds, in world units of sort distance.
/// Panels are ~1 m across, so 0.5 mm per node is invisible geometrically but
/// unambiguous for the sort.
pub const SORT_STEP: f32 = 0.0005;

/// Painter order of one panel node: shared centre + monotonic bias.
#[derive(Component, Clone, Copy, Debug)]
pub struct PanelSort {
    pub center: Vec3,
    pub bias: f32,
}

/// Extracted sort table for the render world.
#[derive(Resource, Default)]
pub struct PanelSortTable {
    pub entries: std::collections::HashMap<MainEntity, (Vec3, f32)>,
}

pub fn extract_panel_sort(
    mut table: ResMut<PanelSortTable>,
    nodes: Extract<Query<(Entity, &PanelSort)>>,
) {
    table.entries.clear();
    for (entity, sort) in nodes.iter() {
        table
            .entries
            .insert(MainEntity::from(entity), (sort.center, sort.bias));
    }
}

pub fn apply_panel_sort(
    table: Res<PanelSortTable>,
    mut phases: ResMut<ViewSortedRenderPhases<Transparent3d>>,
) {
    if table.entries.is_empty() {
        return;
    }
    for phase in phases.0.values_mut() {
        for item in phase.items.values_mut() {
            if let Some((center, bias)) = table.entries.get(&item.main_entity()) {
                item.sorting_info = TransparentSortingInfo3d::Sorted {
                    mesh_center: *center,
                    depth_bias: *bias,
                };
            }
        }
    }
}

/// Installs the sort override in the render app.
pub fn install(app: &mut App) {
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .init_resource::<PanelSortTable>()
        .add_systems(ExtractSchedule, extract_panel_sort)
        .add_systems(
            Render,
            apply_panel_sort
                .in_set(RenderSystems::PhaseSort)
                .before(sort_phase_system::<Transparent3d>),
        );
}
