//! CPU-side frustum culling, producing the `instance_count` for indirect draw.
//!
//! The result is handed to the GPU: `1` draws the whole batch, `0` drops it. Vertices are not rewritten here.

use neo_geo::{Camera, WorldPoint};

use crate::batches::RenderBatch;

/// Whether a batch intersects the viewport (expanded by 64px). If it does, the instance count is 1.
pub fn instance_count(batch: &RenderBatch, camera: &Camera, origin: WorldPoint) -> u32 {
    if batch.indices.is_empty() || batch.vertices.is_empty() {
        return 0;
    }
    let (w, h) = camera.viewport_f64();
    let margin = 64.0;
    let visible = batch.vertices.iter().any(|vertex| {
        let world = WorldPoint::new(
            origin.x + f64::from(vertex.position[0]),
            origin.y + f64::from(vertex.position[1]),
        );
        let screen = camera.world_to_screen(world);
        f64::from(screen[0]) >= -margin
            && f64::from(screen[1]) >= -margin
            && f64::from(screen[0]) <= w + margin
            && f64::from(screen[1]) <= h + margin
    });
    u32::from(visible)
}
