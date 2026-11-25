//! GPU culling: use a compute shader to write 0 into `instance_count` of the indirect parameters.
//!
//! The geometry is still in the original buffer; actually drawing less relies on `draw_indexed_indirect` reading 0 instances.
//! This is the Phase 6 culling path; it does not replace the default `draw_indexed`.

use crate::gpu::RenderError;
use crate::scene_gpu::GpuBatch;

/// Zero the indirect parameter instance count of one batch. The caller then draws with `GpuScene::indirect = true`.
pub fn zero_instance_count(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    batch: &GpuBatch,
) -> Result<(), RenderError> {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("neo-cull-shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/cull.wgsl").into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("neo-cull-bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("neo-cull-bg"),
        layout: &layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: batch.indirect.as_entire_binding(),
        }],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("neo-cull-layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("neo-cull-pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("cs_main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("neo-cull-encoder"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("neo-cull-pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    queue.submit([encoder.finish()]);
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| RenderError::Backend(e.to_string()))?;
    Ok(())
}
