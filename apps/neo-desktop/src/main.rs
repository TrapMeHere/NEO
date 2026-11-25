//! NEO desktop application entry point.
//!
//! This file only does three things: initialize logging, build the eframe window options, and create [`NeoApp`].
//! All business logic lives in `crates/*`; this file stays "thin" (one of ADR-0001's verification points).

// Windows release builds use the GUI subsystem: double-clicking the exe no longer pops up a black console window.
// Debug builds keep the console subsystem so tracing logs can be read directly.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod map_callback;
mod sidecar;

use app::NeoApp;

/// File log writer (NEO_PERF_LOG is used for performance profiling; avoids relying on shell redirection).
struct FileWriter(std::sync::Arc<std::sync::Mutex<std::fs::File>>);

impl std::io::Write for FileWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Keep writing logs even if the lock is poisoned, instead of letting a single log call panic the whole process.
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
    // Write a diagnosable file on any panic (release has no console).
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

    // English is the default UI language. A saved choice wins over the OS locale unless
    // `NEO_LANG` forces a language.
    let language = if std::env::var_os("NEO_LANG").is_some() {
        app::locale::startup_language()
    } else {
        app::saved_language().unwrap_or_else(app::locale::startup_language)
    };
    neo_i18n::set_language(language);

    // Window/taskbar icon (the exe file icon is embedded by build.rs from assets/neo.ico).
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1600.0, 1000.0])
        .with_min_inner_size([960.0, 600.0])
        .with_title(neo_i18n::t("app.window_title"));
    if let Some(icon) = load_window_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        // MSAA on the wgpu side: 0 = off. Must match `MapGpu`'s sample_count (see map_callback).
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
    // Release uses the GUI subsystem (no console); on startup failure fall back to a dialog + error file so a double-click doesn't silently do nothing.
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

/// Decode the window icon from the built-in PNG.
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
