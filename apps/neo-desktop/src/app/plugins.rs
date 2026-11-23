//! NeoApp::plugins 方法。行为与拆分前的 app.rs 相同。

use neo_i18n::{t, tf};
use std::path::Path;

use super::{NeoApp, plugins_dir};

impl NeoApp {
    pub(super) fn run_plugin(&mut self, id: &str) {
        let Some(view) = self.plugins.plugins.iter().find(|plugin| plugin.id == id) else {
            return;
        };
        let path = std::path::PathBuf::from(&view.path);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.plugins.status = tf("plugins.error", &[("err", &err.to_string())]);
                return;
            }
        };
        let package = match neo_plugin::open_package(&bytes, self.plugins.developer_mode) {
            Ok(package) => package,
            Err(err) => {
                self.plugins.status = tf("plugins.error", &[("err", &err.to_string())]);
                return;
            }
        };
        let hosts: Vec<&str> = package
            .manifest
            .capabilities
            .network
            .iter()
            .map(String::as_str)
            .collect();
        let granted = neo_plugin::CapabilitySet::grant_all(&hosts);
        let config = neo_plugin::PluginHostConfig {
            developer_mode: self.plugins.developer_mode,
            ..neo_plugin::PluginHostConfig::default()
        };
        let mut host = match neo_plugin::PluginHost::new(config) {
            Ok(host) => host,
            Err(err) => {
                self.plugins.status = tf("plugins.error", &[("err", &err.to_string())]);
                return;
            }
        };
        if let Err(err) = host.load_binary(&package.manifest_text, &package.entry, &granted) {
            self.plugins.status = tf("plugins.error", &[("err", &err.to_string())]);
            return;
        }
        let commands = host.drain_commands();
        let mut applied = 0usize;
        for command in &commands {
            if neo_plugin::apply_command(&mut self.editor, command).is_ok() {
                applied += 1;
            }
        }
        self.plugin_host = Some(host);
        self.scene = None;
        self.plugins.status = tf("plugins.started", &[("id", id), ("n", &applied.to_string())]);
    }

    pub(super) fn refresh_plugins(&mut self) {
        self.plugins.plugins.clear();
        let dir = plugins_dir();
        let _ = std::fs::create_dir_all(&dir);
        let mut invalid = 0usize;
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("neopkg") {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&path) else {
                    invalid += 1;
                    continue;
                };
                let Ok(package) = neo_plugin::open_package(&bytes, self.plugins.developer_mode) else {
                    invalid += 1;
                    continue;
                };
                let caps = &package.manifest.capabilities;
                let mut capabilities = Vec::new();
                if caps.command {
                    capabilities.push("command");
                }
                if caps.ui {
                    capabilities.push("ui");
                }
                if caps.filesystem {
                    capabilities.push("files");
                }
                if caps.imagery {
                    capabilities.push("imagery");
                }
                if !caps.network.is_empty() {
                    capabilities.push("network");
                }
                if !caps.events.is_empty() {
                    capabilities.push("events");
                }
                let id = package.manifest.id.clone();
                self.plugins.plugins.push(neo_ui::PluginView {
                    id: id.clone(),
                    name: package.manifest.name,
                    version: package.manifest.version,
                    api: package.manifest.api,
                    capabilities: if capabilities.is_empty() {
                        "—".to_owned()
                    } else {
                        capabilities.join(" · ")
                    },
                    signed: !package.manifest.signature.is_empty(),
                    enabled: self.enabled_plugins.contains(&id),
                    path: path.display().to_string(),
                });
            }
        }
        self.plugins.plugins.sort_by(|a, b| a.name.cmp(&b.name));
        if self
            .plugins
            .selected
            .as_ref()
            .is_none_or(|id| !self.plugins.plugins.iter().any(|plugin| &plugin.id == id))
        {
            self.plugins.selected = self.plugins.plugins.first().map(|plugin| plugin.id.clone());
        }
        self.plugins.status = tf(
            "plugins.loaded",
            &[
                ("n", &self.plugins.plugins.len().to_string()),
                ("invalid", &invalid.to_string()),
            ],
        );
    }

    pub(super) fn install_plugin(&mut self, path: &Path) {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.plugins.status = tf("plugins.error", &[("err", &err.to_string())]);
                return;
            }
        };
        if let Err(err) = neo_plugin::open_package(&bytes, self.plugins.developer_mode) {
            self.plugins.status = tf("plugins.error", &[("err", &err.to_string())]);
            return;
        }
        let dir = plugins_dir();
        let _ = std::fs::create_dir_all(&dir);
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "plugin.neopkg".to_owned());
        match std::fs::write(dir.join(&name), &bytes) {
            Ok(()) => {
                self.refresh_plugins();
                self.plugins.status = tf("plugins.installed", &[("name", &name)]);
            }
            Err(err) => {
                self.plugins.status = tf("plugins.error", &[("err", &err.to_string())]);
            }
        }
    }

    pub(super) fn remove_plugin(&mut self, id: &str) {
        let dir = plugins_dir();
        let Some(view) = self
            .plugins
            .plugins
            .iter()
            .find(|plugin| plugin.id == id)
            .cloned()
        else {
            return;
        };
        let path = std::path::PathBuf::from(&view.path);
        if path.parent() != Some(dir.as_path()) {
            self.plugins.status = t("plugins.invalid_path").to_owned();
            return;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {
                self.enabled_plugins.remove(id);
                self.refresh_plugins();
                self.persist_state();
                self.plugins.status = tf("plugins.removed", &[("id", id)]);
            }
            Err(err) => {
                self.plugins.status = tf("plugins.error", &[("err", &err.to_string())]);
            }
        }
    }
}
