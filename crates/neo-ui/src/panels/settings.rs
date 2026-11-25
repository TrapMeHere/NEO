//! settings 面板。

use egui::{Color32, RichText, Ui};
use egui_phosphor::regular as ph;
use neo_i18n::{t, tf};

use super::UiActions;

/// 设置分类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsTab {
    /// 常规（语言、自动保存）。
    General,
    /// 语言。
    Language,
    /// OSM 账户（OAuth）。
    Account,
    /// 编辑（简化容差等）。
    Editing,
    /// 校验（上传门禁）。
    Validation,
    /// 网络（API 地址）。
    Network,
    /// 存储（数据目录、自动保存缓存）。
    Storage,
    /// 快捷键。
    Shortcuts,
}

/// 设置窗口中的账户项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountView {
    /// 稳定 id（OSM 用户 id）。
    pub id: String,
    /// 登录名/显示名。
    pub display_name: String,
    /// 是否为当前账户。
    pub active: bool,
}

/// 可重绑定的快捷键动作（顺序即设置页显示顺序）。
pub const SHORTCUT_ACTIONS: &[&str] = &[
    "undo",
    "redo",
    "copy",
    "paste",
    "duplicate",
    "find",
    "save_xml",
    "finish_way",
    "delete",
    "mode_select",
    "mode_draw_point",
    "mode_draw_line",
    "mode_draw_area",
    "mode_measure",
    "zoom_in",
    "zoom_out",
    "reset_view",
];

fn default_shortcuts() -> std::collections::BTreeMap<String, String> {
    [
        ("undo", "Ctrl+Z"),
        ("redo", "Ctrl+Y"),
        ("copy", "Ctrl+C"),
        ("paste", "Ctrl+V"),
        ("duplicate", "Ctrl+D"),
        ("find", "Ctrl+F"),
        ("save_xml", "Ctrl+S"),
        ("finish_way", "Enter"),
        ("delete", "Delete"),
        ("mode_select", "S"),
        ("mode_draw_point", "1"),
        ("mode_draw_line", "2"),
        ("mode_draw_area", "3"),
        ("mode_measure", "M"),
        ("zoom_in", "="),
        ("zoom_out", "-"),
        ("reset_view", "Home"),
    ]
    .into_iter()
    .map(|(action, binding)| (action.to_owned(), binding.to_owned()))
    .collect()
}

/// 设置窗口的 UI 状态（由应用层持有，跨帧保留）。
#[derive(Clone, Debug)]
pub struct SettingsUiState {
    /// 当前分类。
    pub tab: SettingsTab,
    /// OAuth client_id。
    pub client_id: String,
    /// 回调地址。
    pub redirect_uri: String,
    /// 登录状态/错误提示。
    pub login_status: String,
    /// 已保存账户。
    pub accounts: Vec<AccountView>,
    /// 当前选中的账户 id。
    pub selected: Option<String>,
    /// 自动保存间隔（秒）。
    pub autosave_secs: u64,
    /// 关闭图层/应用时若有未保存编辑则提示。
    pub confirm_unsaved: bool,
    /// way 简化容差（屏幕像素）。
    pub simplify_px: f64,
    /// 绘制时几何吸附。
    pub draw_snap_enabled: bool,
    /// 吸附半径（屏幕像素）。
    pub draw_snap_radius_px: f32,
    /// 数据目录（只读展示）。
    pub data_dir: String,
    /// OSM API 基地址。
    pub api_base: String,
    /// 自定义快捷键：action id → 绑定字符串。
    pub shortcuts: std::collections::BTreeMap<String, String>,
    /// 正在等待按键的 action。
    pub rebinding: Option<String>,
}

impl Default for SettingsUiState {
    fn default() -> Self {
        Self {
            tab: SettingsTab::Language,
            client_id: String::new(),
            redirect_uri: "http://127.0.0.1:8765/callback".to_owned(),
            login_status: String::new(),
            accounts: Vec::new(),
            selected: None,
            autosave_secs: 60,
            confirm_unsaved: true,
            simplify_px: 1.5,
            draw_snap_enabled: true,
            draw_snap_radius_px: 16.0,
            data_dir: String::new(),
            api_base: "https://api.openstreetmap.org".to_owned(),
            shortcuts: default_shortcuts(),
            rebinding: None,
        }
    }
}

/// 设置窗口：左侧分类，右侧内容。
pub fn settings_window(
    ctx: &egui::Context,
    actions: &mut UiActions,
    state: &mut SettingsUiState,
    open: &mut bool,
) {
    egui::Window::new(format!("{} {}", ph::GEAR, t("settings.title")))
        .open(open)
        .collapsible(false)
        .resizable(true)
        .default_size([720.0, 560.0])
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.set_min_width(150.0);
                ui.vertical(|ui| {
                    ui.set_width(140.0);
                    for (tab, icon, key) in [
                        (SettingsTab::General, ph::GEAR, "settings.tab.general"),
                        (SettingsTab::Language, ph::GLOBE, "settings.tab.language"),
                        (SettingsTab::Account, ph::USER, "settings.tab.account"),
                        (SettingsTab::Editing, ph::PENCIL_SIMPLE, "settings.tab.editing"),
                        (
                            SettingsTab::Validation,
                            ph::CHECK_SQUARE,
                            "settings.tab.validation",
                        ),
                        (SettingsTab::Network, ph::GLOBE, "settings.tab.network"),
                        (SettingsTab::Storage, ph::FOLDER_SIMPLE, "settings.tab.storage"),
                        (SettingsTab::Shortcuts, ph::KEYBOARD, "settings.tab.shortcuts"),
                    ] {
                        if ui
                            .selectable_label(state.tab == tab, format!("{icon}  {}", t(key)))
                            .clicked()
                        {
                            state.tab = tab;
                        }
                        ui.add_space(4.0);
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    egui::ScrollArea::vertical().max_height(500.0).show(ui, |ui| {
                        ui.set_min_width(500.0);
                        match state.tab {
                            SettingsTab::General => settings_general(ui, actions, state),
                            SettingsTab::Language => settings_language(ui, actions),
                            SettingsTab::Account => settings_account(ui, actions, state),
                            SettingsTab::Editing => settings_editing(ui, state),
                            SettingsTab::Validation => settings_validation(ui, actions, state),
                            SettingsTab::Network => settings_network(ui, state),
                            SettingsTab::Storage => settings_storage(ui, actions, state),
                            SettingsTab::Shortcuts => settings_shortcuts(ui, state),
                        }
                    });
                });
            });
        });
}

fn settings_general(ui: &mut Ui, actions: &mut UiActions, state: &mut SettingsUiState) {
    ui.heading(t("settings.general"));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        for (lang, key) in [
            (neo_i18n::Lang::ZhCn, "menu.language.zh"),
            (neo_i18n::Lang::ZhTw, "menu.language.zh_tw"),
            (neo_i18n::Lang::EnUs, "menu.language.en"),
        ] {
            if ui
                .selectable_label(neo_i18n::current_language() == lang, t(key))
                .clicked()
            {
                actions.set_language = Some(lang);
            }
        }
    });
    ui.add_space(8.0);
    ui.label(t("settings.general.autosave"));
    ui.add(
        egui::Slider::new(&mut state.autosave_secs, 10..=600)
            .suffix(" s")
            .clamping(egui::SliderClamping::Always),
    );
    ui.add_space(8.0);
    ui.checkbox(&mut state.confirm_unsaved, t("settings.general.confirm_unsaved"));
}

fn settings_editing(ui: &mut Ui, state: &mut SettingsUiState) {
    ui.heading(t("settings.editing"));
    ui.add_space(6.0);
    ui.label(t("settings.editing.simplify"));
    ui.add(
        egui::Slider::new(&mut state.simplify_px, 0.5..=5.0)
            .suffix(" px")
            .clamping(egui::SliderClamping::Always),
    );
    ui.add_space(8.0);
    ui.checkbox(&mut state.draw_snap_enabled, t("settings.editing.draw_snap"));
    ui.label(t("settings.editing.draw_snap_radius"));
    ui.add(
        egui::Slider::new(&mut state.draw_snap_radius_px, 4.0..=48.0)
            .suffix(" px")
            .clamping(egui::SliderClamping::Always),
    );
}

fn settings_validation(ui: &mut Ui, actions: &mut UiActions, _state: &mut SettingsUiState) {
    ui.heading(t("settings.validation"));
    ui.add_space(6.0);
    if ui
        .button(format!("{} {}", ph::CHECK_SQUARE, t("validate.run")))
        .clicked()
    {
        actions.run_validation = true;
    }
}

fn settings_network(ui: &mut Ui, state: &mut SettingsUiState) {
    ui.heading(t("settings.network"));
    ui.add_space(6.0);
    ui.label(t("settings.network.api"));
    ui.add(
        egui::TextEdit::singleline(&mut state.api_base)
            .desired_width(420.0)
            .hint_text("https://api.openstreetmap.org"),
    );
}

fn settings_storage(ui: &mut Ui, actions: &mut UiActions, state: &SettingsUiState) {
    ui.heading(t("settings.storage"));
    ui.add_space(6.0);
    ui.label(t("settings.storage.data_dir"));
    ui.add(egui::Label::new(RichText::new(&state.data_dir).monospace().size(11.5)).wrap());
    ui.horizontal(|ui| {
        if ui
            .button(format!("{} {}", ph::FOLDER_OPEN, t("settings.storage.open")))
            .clicked()
        {
            actions.open_data_dir = true;
        }
        if ui
            .button(format!("{} {}", ph::TRASH, t("settings.storage.clear_autosave")))
            .clicked()
        {
            actions.clear_autosave = true;
        }
    });
}

fn settings_language(ui: &mut Ui, actions: &mut UiActions) {
    ui.heading(t("settings.language"));
    ui.add_space(6.0);
    for (lang, key) in [
        (neo_i18n::Lang::ZhCn, "menu.language.zh"),
        (neo_i18n::Lang::ZhTw, "menu.language.zh_tw"),
        (neo_i18n::Lang::EnUs, "menu.language.en"),
    ] {
        if ui
            .selectable_label(neo_i18n::current_language() == lang, t(key))
            .clicked()
        {
            actions.set_language = Some(lang);
        }
    }
}

fn settings_account(ui: &mut Ui, actions: &mut UiActions, state: &mut SettingsUiState) {
    ui.heading(t("settings.account"));
    ui.add_space(4.0);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !state.client_id.trim().is_empty(),
                egui::Button::new(format!("{}  {}", ph::SIGN_IN, t("settings.account.login"))),
            )
            .clicked()
        {
            actions.oauth_login = true;
        }
        if ui
            .add_enabled(
                state.selected.is_some(),
                egui::Button::new(format!("{}  {}", ph::SIGN_OUT, t("settings.account.logout"))),
            )
            .clicked()
        {
            actions.oauth_logout = true;
        }
    });
    if !state.login_status.is_empty() {
        ui.add_space(2.0);
        ui.label(
            RichText::new(&state.login_status)
                .size(11.5)
                .color(Color32::from_rgb(180, 210, 170)),
        );
    }
    ui.separator();
    if state.accounts.is_empty() {
        ui.label(
            RichText::new(t("settings.account.empty"))
                .size(11.5)
                .color(Color32::from_rgb(150, 160, 174)),
        );
    } else {
        for account in &state.accounts {
            ui.horizontal(|ui| {
                let label = if account.active {
                    format!("{} {}  (active)", ph::CHECK, account.display_name)
                } else {
                    format!("{} {}", ph::USER, account.display_name)
                };
                if ui.selectable_label(account.active, label).clicked() {
                    actions.oauth_select = Some(account.id.clone());
                }
                if ui
                    .add(egui::Button::new(RichText::new(ph::TRASH).size(12.0)).frame(false))
                    .clicked()
                {
                    actions.oauth_remove = Some(account.id.clone());
                }
            });
        }
    }
}

enum ShortcutCapture {
    Binding(String),
    Cancel,
}

fn capture_shortcut(ctx: &egui::Context) -> Option<ShortcutCapture> {
    ctx.input(|input| {
        for key in egui::Key::ALL {
            if !input.key_pressed(*key) {
                continue;
            }
            if *key == egui::Key::Escape {
                return Some(ShortcutCapture::Cancel);
            }
            let mut parts = Vec::new();
            if input.modifiers.command || input.modifiers.ctrl {
                parts.push("Ctrl".to_owned());
            }
            if input.modifiers.shift {
                parts.push("Shift".to_owned());
            }
            if input.modifiers.alt {
                parts.push("Alt".to_owned());
            }
            parts.push(key.name().to_owned());
            return Some(ShortcutCapture::Binding(parts.join("+")));
        }
        None
    })
}

fn shortcut_action_key(action: &str) -> &'static str {
    match action {
        "undo" => "shortcut.action.undo",
        "redo" => "shortcut.action.redo",
        "copy" => "shortcut.action.copy",
        "paste" => "shortcut.action.paste",
        "duplicate" => "shortcut.action.duplicate",
        "find" => "shortcut.action.find",
        "save_xml" => "shortcut.action.save_xml",
        "finish_way" => "shortcut.action.finish_way",
        "delete" => "shortcut.action.delete",
        "mode_select" => "shortcut.action.mode_select",
        "mode_draw_point" => "shortcut.action.mode_draw_point",
        "mode_draw_line" | "mode_draw" => "shortcut.action.mode_draw_line",
        "mode_draw_area" => "shortcut.action.mode_draw_area",
        "mode_measure" => "shortcut.action.mode_measure",
        "zoom_in" => "shortcut.action.zoom_in",
        "zoom_out" => "shortcut.action.zoom_out",
        "reset_view" => "shortcut.action.reset_view",
        _ => "shortcut.action.unknown",
    }
}

fn settings_shortcuts(ui: &mut Ui, state: &mut SettingsUiState) {
    ui.heading(t("settings.shortcuts"));
    if let Some(action) = &state.rebinding {
        ui.label(
            RichText::new(tf(
                "settings.shortcuts_capture",
                &[("action", t(shortcut_action_key(action)))],
            ))
            .color(Color32::from_rgb(235, 190, 100)),
        );
        match capture_shortcut(ui.ctx()) {
            Some(ShortcutCapture::Binding(binding)) => {
                state.shortcuts.insert(action.clone(), binding);
                state.rebinding = None;
            }
            Some(ShortcutCapture::Cancel) => state.rebinding = None,
            None => {}
        }
    }
    egui::Grid::new("settings_shortcuts_grid")
        .num_columns(3)
        .spacing([14.0, 4.0])
        .show(ui, |ui| {
            for action in SHORTCUT_ACTIONS {
                ui.label(t(shortcut_action_key(action)));
                ui.monospace(state.shortcuts.get(*action).map(String::as_str).unwrap_or("—"));
                if ui.small_button(t("settings.shortcuts_rebind")).clicked() {
                    state.rebinding = Some((*action).to_owned());
                }
                ui.end_row();
            }
        });
    ui.add_space(4.0);
    if ui.button(t("settings.shortcuts_reset")).clicked() {
        state.shortcuts = default_shortcuts();
        state.rebinding = None;
    }
}
