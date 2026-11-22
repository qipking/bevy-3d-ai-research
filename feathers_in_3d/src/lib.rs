//! `feathers_in_3d`: draw a `bevy_ui` / `bevy_feathers` subtree as world-space panels.
//!
//! Route B, packaged:
//! * `world3d` mirrors every laid-out node into geometry using bevy_luma's
//!   `UNodeMaterial3d` (SDF rounded corners, borders, screen-derivative AA).
//! * `text3d` turns `bevy_text`'s laid-out glyphs into quads on the font atlas,
//!   so feathers text (including styled `TextSpan`s) is real 3D text.
//!
//! Add [`FeathersIn3dPlugin`], then put [`WorldPanel`] + `MirrorRoot` on a UI
//! subtree root. The subtree must be laid out somewhere (it may be off-screen —
//! that is how you avoid the 2D overlay drawing on top of the 3D copy).

mod sort;
mod text3d;
mod world3d;

pub use sort::{PanelSort, SORT_STEP};
pub use text3d::{GlyphMaterials, GlyphMeshChild, PanelTextMeshes, TextDirty, mark_text_dirty, sync_panel_text};
pub use world3d::{MirrorRoot, PanelNode, WorldPanel, Z_BIAS_PER_LEVEL, sync_mirror};

use bevy::prelude::*;

/// Adds luma's SDF 3D material and the two mirror systems.
pub struct FeathersIn3dPlugin;

impl Plugin for FeathersIn3dPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bevy_luma::engine::layout::render::UnivisRenderPlugin)
            .init_resource::<GlyphMaterials>()
            .init_resource::<TextDirty>()
            .add_systems(PostUpdate, (mark_text_dirty, sync_mirror, sync_panel_text).chain());
        sort::install(app);
    }
}
