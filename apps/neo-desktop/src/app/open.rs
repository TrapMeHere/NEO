//! NeoApp::open 方法。行为与拆分前的 app.rs 相同。

use neo_editor::{AddNode, AddWay, Editor};
use neo_i18n::{t, tf};
use neo_io::{import_pbf_with_report, read_gpx, read_osm_xml_file};
use neo_store::DataSet;
use std::path::Path;
use std::sync::mpsc::TryRecvError;

use super::NeoApp;

impl NeoApp {
    ///
    /// GPX 导入为当前图层的节点/way（通过命令批，保持单写者）
    pub(super) fn import_gpx(&mut self, path: &Path) {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) => {
                self.status = tf("app.error.gpx", &[("err", &err.to_string())]);
                return;
            }
        };
        let mut temp = DataSet::new();
        let report = match read_gpx(&text, &mut temp) {
            Ok(report) => report,
            Err(err) => {
                self.status = tf("app.error.gpx", &[("err", &err.to_string())]);
                return;
            }
        };
        let mut node_map = std::collections::HashMap::new();
        let mut commands: Vec<Box<dyn neo_editor::Command>> = Vec::new();
        for node in temp.iter_nodes() {
            let new_id = self.editor.next_local_id();
            node_map.insert(node.id, new_id);
            let tags = temp
                .tags_of(node.tags)
                .iter()
                .map(|(key, value)| (temp.resolve(key).to_owned(), temp.resolve(value).to_owned()))
                .collect();
            let mut cmd = AddNode::new(node.pos, tags);
            cmd.id = Some(new_id);
            commands.push(Box::new(cmd));
        }
        for way in temp.iter_ways() {
            let new_id = self.editor.next_local_id();
            let nodes = temp
                .way_node_ids(way.handle)
                .iter()
                .map(|id| node_map.get(id).copied().unwrap_or(*id))
                .collect();
            let tags = temp
                .tags_of(way.tags)
                .iter()
                .map(|(key, value)| (temp.resolve(key).to_owned(), temp.resolve(value).to_owned()))
                .collect();
            let mut cmd = AddWay::new(nodes, tags);
            cmd.id = Some(new_id);
            commands.push(Box::new(cmd));
        }
        if commands.is_empty() {
            self.status = t("app.status.gpx_empty").to_owned();
            return;
        }
        match self.editor.execute_batch(commands) {
            Ok(()) => {
                self.scene = None;
                self.status = tf(
                    "app.status.gpx_imported",
                    &[
                        ("points", &report.track_points.to_string()),
                        ("tracks", &report.tracks.to_string()),
                    ],
                );
            }
            Err(err) => {
                self.status = tf("app.error.gpx", &[("err", &err.to_string())]);
            }
        }
    }

    /// 后台打开 PBF/XML：大文件导入不再冻结 UI；完成后始终作为新的活动图层。
    pub(super) fn start_open_dataset(&mut self, path: std::path::PathBuf, pbf: bool) {
        if self.open_busy {
            self.status = t("app.status.import_busy").to_owned();
            return;
        }
        self.open_busy = true;
        self.status = tf("app.status.importing", &[("path", &path.display().to_string())]);
        let (tx, rx) = std::sync::mpsc::channel();
        self.open_rx = Some(rx);
        std::thread::spawn(move || {
            let label = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            let result = if pbf {
                import_pbf_with_report(&path)
                    .map(|(data, report)| (data, label, report.to_string(), path.clone()))
                    .map_err(|err| err.to_string())
            } else {
                let mut data = DataSet::new();
                read_osm_xml_file(&mut data, &path)
                    .map(|report| (data, label, report.to_string(), path.clone()))
                    .map_err(|err| err.to_string())
            };
            let _ = tx.send(result);
        });
    }

    pub(super) fn poll_open_dataset(&mut self) {
        let Some(rx) = &self.open_rx else { return };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("import worker disconnected".to_owned()),
        };
        self.open_rx = None;
        self.open_busy = false;
        match result {
            Ok((data, label, report, path)) => {
                if self.data_listed {
                    self.park_active_layer();
                }
                self.data_path = Some(path);
                self.layer_visible = true;
                self.layer_active = true;
                self.set_editor(Editor::new(data), label);
                self.reopen_journal();
                self.status = report;
            }
            Err(err) => {
                self.status = tf("app.error.import_detail", &[("err", &err)]);
            }
        }
    }
}
