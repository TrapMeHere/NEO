//! wgpu 设备初始化（headless）与错误类型。
//!
//! 窗口/表面（surface）由 eframe/egui-wgpu 管理；这里只创建"无窗口"设备用于：
//!
//! * 性能验证原型的离屏渲染（PNG 输出）；
//! * 基准测试（GPU 帧时间）；
//! * CI 里的 GPU 测试。
//!
//! 适配器选择：优先独显（HighPerformance），可用环境变量覆盖：
//! `WGPU_BACKEND`（dx12/vulkan/gl）、`WGPU_ADAPTER_NAME`（名字子串）。

use wgpu::util::DeviceExt as _;

/// 渲染层错误。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RenderError {
    /// 找不到可用的 GPU 适配器。
    #[error("no GPU adapter available: {0}")]
    NoAdapter(String),
    /// 创建设备失败。
    #[error("device creation failed: {0}")]
    Device(String),
    /// wgpu 校验/后端错误。
    #[error("wgpu backend error: {0}")]
    Backend(String),
    /// 读回映射失败。
    #[error("buffer map failed: {0}")]
    Map(String),
    /// PNG 写入失败。
    #[error("png write failed: {0}")]
    Image(String),
    /// IO 错误。
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// headless GPU 上下文。
pub struct GpuContext {
    /// wgpu 实例。
    pub instance: wgpu::Instance,
    /// 适配器。
    pub adapter: wgpu::Adapter,
    /// 设备。
    pub device: wgpu::Device,
    /// 队列。
    pub queue: wgpu::Queue,
    /// 适配器信息（名字/后端/驱动）。
    pub info: wgpu::AdapterInfo,
    /// 设备是否支持时间戳查询（GPU 计时）。
    pub timestamp_query_supported: bool,
}

impl GpuContext {
    /// 创建 headless 上下文（离屏渲染不需要 surface）。
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

    /// 一行描述（用于日志/基准报告）。
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

    /// 创建只读 staging buffer 并读回内容（同步等待）。
    ///
    /// 调用方负责保证 buffer 的 usage 含 `COPY_DST`。
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
        // 复制出来，避免 BufferView 与 unmap 冲突。
        let mapped = slice
            .get_mapped_range()
            .map_err(|e| RenderError::Map(e.to_string()))?;
        let out = mapped[..(size as usize).min(mapped.len())].to_vec();
        drop(mapped);
        buffer.unmap();
        Ok(out)
    }
}

/// 便捷：创建一个可初始化的顶点/索引 buffer。
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
