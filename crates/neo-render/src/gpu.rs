//! wgpu device initialization (headless) and error types.
//!
//! Window/surface handling lives in eframe/egui-wgpu; here we only create a "windowless" device for:
//!
//! * offscreen rendering for the performance verification prototype (PNG output);
//! * benchmarks (GPU frame time);
//! * GPU tests in CI.
//!
//! Adapter selection: prefer the discrete GPU (HighPerformance), overridable via environment variables:
//! `WGPU_BACKEND` (dx12/vulkan/gl), `WGPU_ADAPTER_NAME` (name substring).

use wgpu::util::DeviceExt as _;

/// Render-layer error.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RenderError {
    /// No usable GPU adapter found.
    #[error("no GPU adapter available: {0}")]
    NoAdapter(String),
    /// Device creation failed.
    #[error("device creation failed: {0}")]
    Device(String),
    /// wgpu validation/backend error.
    #[error("wgpu backend error: {0}")]
    Backend(String),
    /// Readback mapping failed.
    #[error("buffer map failed: {0}")]
    Map(String),
    /// PNG write failed.
    #[error("png write failed: {0}")]
    Image(String),
    /// IO error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Headless GPU context.
pub struct GpuContext {
    /// wgpu instance.
    pub instance: wgpu::Instance,
    /// Adapter.
    pub adapter: wgpu::Adapter,
    /// Device.
    pub device: wgpu::Device,
    /// Queue.
    pub queue: wgpu::Queue,
    /// Adapter info (name/backend/driver).
    pub info: wgpu::AdapterInfo,
    /// Whether the device supports timestamp queries (GPU timing).
    pub timestamp_query_supported: bool,
}

impl GpuContext {
    /// Create a headless context (offscreen rendering does not need a surface).
    pub fn new_headless() -> Result<Self, RenderError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .map_err(|e| RenderError::NoAdapter(e.to_string()))?;

        let info = adapter.get_info();
        let timestamp_query_supported = adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("neo-device"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .map_err(|e| RenderError::Device(e.to_string()))?;

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            info,
            timestamp_query_supported,
        })
    }

    /// One-line description (for logs/benchmark reports).
    pub fn describe(&self) -> String {
        format!(
            "{} | backend={:?} | device_type={:?} | driver={} {} | timestamp_query={}",
            self.info.name,
            self.info.backend,
            self.info.device_type,
            self.info.driver,
            self.info.driver_info,
            self.timestamp_query_supported
        )
    }

    /// Create a read-only staging buffer and read its contents back (synchronous wait).
    ///
    /// The caller is responsible for ensuring the buffer usage includes `COPY_DST`.
    pub fn read_buffer(&self, buffer: &wgpu::Buffer, size: u64) -> Result<Vec<u8>, RenderError> {
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RenderError::Backend(e.to_string()))?;
        rx.recv()
            .map_err(|e| RenderError::Map(e.to_string()))?
            .map_err(|e| RenderError::Map(e.to_string()))?;
        // Copy it out to avoid conflicting with unmap through BufferView.
        let mapped = slice
            .get_mapped_range()
            .map_err(|e| RenderError::Map(e.to_string()))?;
        let out = mapped[..(size as usize).min(mapped.len())].to_vec();
        drop(mapped);
        buffer.unmap();
        Ok(out)
    }
}

/// Convenience: create a vertex/index buffer that can be initialized.
pub fn create_buffer_init(
    device: &wgpu::Device,
    label: &str,
    contents: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents,
        usage,
    })
}
