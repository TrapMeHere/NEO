//! imagery 面板。

use egui::{Color32, RichText};
use egui_phosphor::regular as ph;
use neo_i18n::{t, tf};

use super::UiActions;

/// 影像图层 UI 状态。
#[derive(Clone, Debug)]
pub struct ImageryUiState {
    /// 是否显示。
    pub enabled: bool,
    /// TMS 模板（`{z}/{x}/{y}` 或 `{-y}`）。
    pub url: String,
    /// 不透明度 0..1。
    pub opacity: f32,
    /// 状态/错误。
    pub status: String,
    /// 已缓存瓦片数。
    pub cached: usize,
    /// 影像向东偏移（屏幕像素）。
    pub offset_x: f32,
    /// 影像向南偏移（屏幕像素）。
    pub offset_y: f32,
    /// WMS 模式（单张 GetMap）而不是 TMS 瓦片。
    pub wms: bool,
    /// WMS layers 参数。
    pub wms_layers: String,
    /// 当前预设 id。空表示自定义 URL。
    pub preset_id: String,
    /// `{apikey}` 的替换值，只留在本机。
    pub api_key: String,
    /// 影像署名。
    pub attribution: String,
    /// 只列出全球影像和当前国家的影像。
    pub region_only: bool,
    /// 预设搜索。
    pub query: String,
    /// WMS 投影，例如 `EPSG:3857`。
    pub wms_crs: String,
    /// WMS 1.3 地理坐标时 bbox 纬度在前。
    pub wms_lat_first: bool,
}

impl Default for ImageryUiState {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            opacity: 0.85,
            status: String::new(),
            cached: 0,
            offset_x: 0.0,
            offset_y: 0.0,
            wms: false,
            wms_layers: String::new(),
            preset_id: String::new(),
            api_key: String::new(),
            attribution: String::new(),
            region_only: true,
            query: String::new(),
            wms_crs: String::new(),
            wms_lat_first: false,
        }
    }
}

/// 影像图层窗口。
pub fn imagery_window(
    ctx: &egui::Context,
    actions: &mut UiActions,
    open: &mut bool,
    state: &mut ImageryUiState,
    lat: f64,
    lon: f64,
) {
    if !*open {
        return;
    }
    egui::Window::new(format!("{} {}", ph::IMAGE, t("imagery.title")))
        .open(open)
        .resizable(true)
        .default_width(520.0)
        .show(ctx, |ui| {
            ui.checkbox(&mut state.enabled, t("imagery.enabled"));
            preset_row(ui, state, lat, lon);
            ui.horizontal(|ui| {
                ui.radio_value(&mut state.wms, false, t("imagery.tms"));
                ui.radio_value(&mut state.wms, true, t("imagery.wms"));
            });
            if state.url.contains("{apikey}") {
                ui.label(t("imagery.apikey"));
                ui.add(
                    egui::TextEdit::singleline(&mut state.api_key)
                        .desired_width(490.0)
                        .password(true),
                );
            }
            if !state.attribution.is_empty() {
                ui.label(t("imagery.attribution"));
                ui.label(RichText::new(&state.attribution).size(11.0).weak());
            }
            ui.label(t("imagery.url"));
            ui.add(
                egui::TextEdit::singleline(&mut state.url)
                    .desired_width(490.0)
                    .hint_text("https://tile.example/{z}/{x}/{y}.png"),
            );
            if state.wms {
                ui.label(t("imagery.layers"));
                ui.add(
                    egui::TextEdit::singleline(&mut state.wms_layers)
                        .desired_width(490.0)
                        .hint_text("layer1,layer2"),
                );
            }
            ui.add(
                egui::Slider::new(&mut state.opacity, 0.0..=1.0)
                    .text(t("imagery.opacity"))
                    .clamping(egui::SliderClamping::Always),
            );
            ui.horizontal(|ui| {
                ui.label(t("imagery.offset"));
                ui.add(
                    egui::DragValue::new(&mut state.offset_x)
                        .speed(0.5)
                        .range(-200.0..=200.0)
                        .suffix(" px"),
                );
                ui.add(
                    egui::DragValue::new(&mut state.offset_y)
                        .speed(0.5)
                        .range(-200.0..=200.0)
                        .suffix(" px"),
                );
            });
            ui.horizontal(|ui| {
                if ui
                    .button(format!("{} {}", ph::CHECK, t("imagery.apply")))
                    .clicked()
                {
                    actions.imagery_apply = true;
                }
                if ui
                    .button(format!("{} {}", ph::TRASH, t("imagery.clear")))
                    .clicked()
                {
                    actions.imagery_clear = true;
                }
                if ui
                    .button(format!("{} {}", ph::FOLDER_OPEN, t("imagery.offset_load")))
                    .clicked()
                {
                    actions.imagery_offset_load = true;
                }
                if ui
                    .button(format!("{} {}", ph::EXPORT, t("imagery.offset_save")))
                    .clicked()
                {
                    actions.imagery_offset_save = true;
                }
                if ui
                    .button(format!("{} {}", ph::CROSSHAIR, t("imagery.offset_apply")))
                    .clicked()
                {
                    actions.imagery_apply_offset = true;
                }
            });
            ui.label(tf("imagery.cached", &[("n", &state.cached.to_string())]));
            if !state.status.is_empty() {
                ui.separator();
                ui.label(
                    RichText::new(&state.status)
                        .size(11.0)
                        .color(Color32::from_rgb(160, 200, 170)),
                );
            }
        });
}

/// 顶栏快选：当前地区能用的内置影像，再加上最多 6 条该地区优先源。
pub(super) fn quick_imagery_choices(lat: f64, lon: f64) -> Vec<(String, String)> {
    let country = neo_style::imagery::country_at(lat, lon);
    const QUICK: &[&str] = &[
        "esri-world",
        "esri-clarity",
        "bing",
        "mapbox",
        "tianditu",
        "sentinel-2025",
        "sentinel-2020",
    ];
    let mut out = Vec::new();
    for id in QUICK {
        let Some(preset) = neo_style::imagery_presets()
            .iter()
            .find(|preset| preset.id == *id)
        else {
            continue;
        };
        if preset.covers(country) {
            out.push((preset.id.clone(), preset_name(id)));
        }
    }
    let mut extra = 0;
    for preset in neo_style::imagery_presets() {
        if extra >= 6 {
            break;
        }
        if !preset.best || !preset.covers(country) || QUICK.contains(&preset.id.as_str()) {
            continue;
        }
        out.push((preset.id.clone(), preset.name.clone()));
        extra += 1;
    }
    out
}

fn preset_row(ui: &mut egui::Ui, state: &mut ImageryUiState, lat: f64, lon: f64) {
    let country = neo_style::imagery::country_at(lat, lon);
    ui.checkbox(&mut state.region_only, t("imagery.region_only"));
    if let Some(code) = country {
        let (english, chinese) = neo_style::imagery::country_names(code).unwrap_or((code, code));
        let name = if neo_i18n::current_language() == neo_i18n::Lang::ZhCn {
            chinese
        } else {
            english
        };
        ui.label(format!("{}: {name} ({code})", t("imagery.region")));
    } else if state.region_only {
        ui.label(t("imagery.region.unknown"));
    }
    ui.add(
        egui::TextEdit::singleline(&mut state.query)
            .desired_width(360.0)
            .hint_text(t("imagery.search")),
    );
    let query = state.query.trim().to_lowercase();
    let mut shown: Vec<&neo_style::ImageryPreset> = neo_style::imagery_presets()
        .iter()
        .filter(|preset| {
            if state.region_only {
                preset.covers(country)
            } else if query.is_empty() {
                preset.countries.is_empty()
            } else {
                true
            }
        })
        .filter(|preset| preset_matches(preset, &query))
        .collect();
    let hidden = if !state.region_only && shown.len() > 80 {
        let extra = shown.len() - 80;
        shown.truncate(80);
        extra
    } else {
        0
    };
    let mut selected = state.preset_id.clone();
    let current = neo_style::imagery_presets()
        .iter()
        .find(|preset| preset.id == selected)
        .map(preset_label)
        .unwrap_or_else(|| preset_name(""));
    egui::ComboBox::from_label(t("imagery.preset"))
        .selected_text(current)
        .width(360.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut selected, String::new(), preset_name(""));
            for preset in shown {
                ui.selectable_value(&mut selected, preset.id.clone(), preset_label(preset));
            }
        });
    if hidden > 0 {
        ui.label(tf("imagery.more", &[("n", &hidden.to_string())]));
    }
    if selected != state.preset_id {
        state.preset_id = selected;
        if let Some(preset) = neo_style::imagery_presets()
            .iter()
            .find(|preset| preset.id == state.preset_id)
        {
            state.url = preset.url.clone();
            state.wms = preset.kind == neo_style::ImageryKind::Wms;
            state.wms_layers = preset.layers.clone();
            state.attribution = preset.attribution.clone();
            state.wms_crs = preset.crs.clone();
            state.wms_lat_first = preset.bbox_lat_first;
        }
    }
}

fn preset_matches(preset: &neo_style::ImageryPreset, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    preset.name.to_lowercase().contains(query)
        || preset.id.to_lowercase().contains(query)
        || preset
            .countries
            .iter()
            .any(|code| code.to_lowercase().contains(query))
}

fn preset_label(preset: &neo_style::ImageryPreset) -> String {
    let name = match preset.id.as_str() {
        "osm" | "esri-world" | "esri-clarity" | "bing" | "mapbox" | "tianditu" | "sentinel-2025"
        | "sentinel-2020" => preset_name(&preset.id),
        _ => preset.name.clone(),
    };
    let scope = if preset.countries.is_empty() {
        t("imagery.scope.world").to_string()
    } else {
        preset.countries.join(", ")
    };
    format!("{name} · {scope}")
}

fn preset_name(id: &str) -> String {
    match id {
        "osm" => t("imagery.preset.osm").into(),
        "esri-world" => t("imagery.preset.esri").into(),
        "esri-clarity" => t("imagery.preset.esri_clarity").into(),
        "bing" => t("imagery.preset.bing").into(),
        "mapbox" => t("imagery.preset.mapbox").into(),
        "tianditu" => t("imagery.preset.tianditu").into(),
        "sentinel-2025" => t("imagery.preset.sentinel2025").into(),
        "sentinel-2020" => t("imagery.preset.sentinel2020").into(),
        _ => t("imagery.preset.custom").into(),
    }
}
