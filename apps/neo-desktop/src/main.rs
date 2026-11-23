//! NEO 桌面应用入口。
//!
//! 这里只做三件事：初始化日志、构造 eframe 窗口选项、创建 [`NeoApp`]。
//! 业务逻辑全部在 `crates/*`，本文件保持"薄"（ADR-0001 的验证点之一）。

// Windows release 版使用 GUI 子系统：双击 exe 不再弹出黑色控制台窗口。
// debug 构建保留控制台子系统，方便直接看 tracing 日志。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod map_callback;
mod sidecar;

use app::NeoApp;

/// 文件日志 writer（NEO_PERF_LOG 用于性能分析；避免依赖 shell 重定向）。
struct FileWriter(std::sync::Arc<std::sync::Mutex<std::fs::File>>);

impl std::io::Write for FileWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // 锁中毒时依然写日志，而不是让任何一条日志 panic 掉整个进程。
        let mut guard = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.write_all(buf)?;
        guard.flush()?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let mut guard = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.flush()
    }
}

fn main() -> eframe::Result {
    // 任何 panic 都写一份可诊断文件（release 无控制台）。
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let path = std::env::temp_dir().join("NEO").join("neo-panic.txt");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, format!("{info}\n"));
        default_hook(info);
    }));

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("neo=info,warn"));
    if let Ok(path) = std::env::var("NEO_PERF_LOG") {
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::File::create(&path) {
            Ok(file) => {
                let file = std::sync::Arc::new(std::sync::Mutex::new(file));
                let shared = file.clone();
                tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_writer(move || FileWriter(shared.clone()))
                    .init();
            }
            Err(_) => {
                tracing_subscriber::fmt().with_env_filter(filter).init();
            }
        }
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    // 窗口/任务栏图标（exe 文件图标由 build.rs 嵌入 assets/neo.ico）。
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1600.0, 1000.0])
        .with_min_inner_size([960.0, 600.0])
        .with_title(neo_i18n::t("app.window_title"));
    if let Some(icon) = load_window_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        // wgpu 侧 MSAA：0 = off。必须与 `MapGpu` 的 sample_count 一致（见 map_callback）。
        multisampling: 0,
        ..Default::default()
    };

    let result = eframe::run_native(
        "NEO",
        options,
        Box::new(|cc| {
            let app = NeoApp::new(cc)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::from(e.to_string()) })?;
            Ok(Box::new(app))
        }),
    );
    // release 是 GUI 子系统（没有控制台），启动失败时用对话框 + 错误文件兜底，避免"双击没反应"。
    if let Err(err) = &result {
        let message = neo_i18n::tf("app.start_failed", &[("err", &err.to_string())]);
        let path = std::env::temp_dir().join("NEO").join("neo-startup-error.txt");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, &message);
        rfd::MessageDialog::new()
            .set_title(neo_i18n::t("app.start_failed_title"))
            .set_description(&message)
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
    result
}

/// 从内置 PNG 解码窗口图标。
fn load_window_icon() -> Option<egui::IconData> {
    let image = image::load_from_memory(include_bytes!("../../../assets/neo-icon.png"))
        .ok()?
        .to_rgba8();
    let (width, height) = image.dimensions();
    Some(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}
