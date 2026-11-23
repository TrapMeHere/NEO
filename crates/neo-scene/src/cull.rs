//! CPU 侧视锥剔除，产出 indirect draw 的 `instance_count`。
//!
//! 结果交给 GPU：`1` 画整批，`0` 丢掉整批。顶点不在这里重写。

use neo_geo::{Camera, WorldPoint};

use crate::batches::RenderBatch;

/// 批次是否与视口（外扩 64px）相交。相交则实例数为 1。
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
