//! NeoApp::session 方法。行为与拆分前的 app.rs 相同。

use anyhow::Result;
use neo_core::{ObjectId, ObjectKind};
use neo_editor::{Editor, FileJournal};
use neo_i18n::{t, tf};
use neo_io::{XmlWriteMode, write_osm_xml_file};
use neo_persist::{SessionSelection, SnapshotStore};
use std::time::Instant;

use super::{
    AUTOSAVE_KEEP, NeoApp, autosave_dir, data_dir, kind_from_name, load_dataset_file, load_initial_data,
};

impl NeoApp {
    /// 面板所需的只读状态快照
    /// 周期性把 journal 复制
    /// `out/autosave/`（保留最
    /// N 代）
    ///
    /// journal 本身已经持久化每条命令；autosave
    /// *快照副本**，用于磁盘损
    /// 误删
    /// 仍能找回最近的编辑记录（阶
    /// 4：自动保存与保留代数）
    pub(super) fn maybe_autosave(&mut self) {
        if self.last_autosave.elapsed().as_secs() < self.settings.autosave_secs.max(10) {
            return;
        }
        self.last_autosave = Instant::now();
        if !self.journal_path.exists() {
            return;
        }
        use std::sync::atomic::Ordering;
        if self.autosave_busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let journal = self.journal_path.clone();
        let dir = autosave_dir();
        let counts = (
            self.editor.data().node_count(),
            self.editor.data().way_count(),
            self.editor.data().relation_count(),
        );
        let busy = self.autosave_busy.clone();
        // 整份 journal
        // copy/read + SQLite 写入放到后台，避免在 UI 线程
        //
        // journal 增长而变慢的 IO
        std::thread::spawn(move || {
            let _ = std::fs::create_dir_all(&dir);
            let mut entries: Vec<String> = std::fs::read_dir(&dir)
                .map(|iter| {
                    iter.flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .filter(|name| name.starts_with("neo-") && name.ends_with(".jsonl"))
                        .collect()
                })
                .unwrap_or_default();
            entries.sort();
            let next = entries
                .last()
                .and_then(|name| {
                    name.trim_start_matches("neo-")
                        .trim_end_matches(".jsonl")
                        .parse::<u32>()
                        .ok()
                })
                .unwrap_or(0)
                + 1;
            let target = dir.join(format!("neo-{next:04}.jsonl"));
            if std::fs::copy(&journal, &target).is_ok() {
                entries.push(format!("neo-{next:04}.jsonl"));
                while entries.len() > AUTOSAVE_KEEP {
                    let oldest = entries.remove(0);
                    let _ = std::fs::remove_file(dir.join(oldest));
                }
                if let Ok(bytes) = std::fs::read(&journal) {
                    let db = dir.join("neo-snapshots.sqlite");
                    if let Ok(store) = SnapshotStore::open(&db) {
                        let _ =
                            store.record(&format!("autosave #{next}"), &bytes, counts.0, counts.1, counts.2);
                        let _ = store.prune(10);
                    }
                }
            }
            busy.store(false, Ordering::Release);
        });
    }

    pub(super) fn selection_items(&self) -> Vec<SessionSelection> {
        let selection = self.editor.selection();
        let mut items = Vec::with_capacity(selection.len());
        for id in selection.nodes() {
            items.push(SessionSelection {
                kind: "node",
                id: id.0,
            });
        }
        for id in selection.ways() {
            items.push(SessionSelection {
                kind: "way",
                id: id.0,
            });
        }
        for id in selection.relations() {
            items.push(SessionSelection {
                kind: "relation",
                id: id.0,
            });
        }
        items
    }

    pub(super) fn restore_selection(&mut self, items: &[SessionSelection]) {
        self.editor.selection_mut().clear();
        for item in items {
            let Some(kind) = kind_from_name(item.kind) else {
                continue;
            };
            let id = ObjectId(item.id);
            let exists = match kind {
                ObjectKind::Node => self.editor.data().node_handle(id).is_some(),
                ObjectKind::Way => self.editor.data().way_handle(id).is_some(),
                ObjectKind::Relation => self.editor.data().relation_handle(id).is_some(),
                ObjectKind::Changeset => false,
            };
            if exists {
                self.editor.selection_mut().insert(kind, id);
            }
        }
    }

    pub(super) fn reopen_journal(&mut self) {
        match FileJournal::open(&self.journal_path) {
            Ok(sink) => self.editor.set_journal_sink(Box::new(sink)),
            Err(err) => self.status = tf("app.error.journal_open", &[("err", &err.to_string())]),
        }
    }

    pub(super) fn recover_from_journal(&mut self) -> Result<()> {
        if !self.journal_path.exists() {
            self.status = t("app.error.journal_missing").into();
            return Ok(());
        }
        if self.editor.history().is_dirty() {
            // 不允许静默丢弃当前未保存编辑
            self.status = t("app.error.recover_dirty").into();
            return Ok(());
        }
        let (frames, _bad) = neo_editor::read_journal_report(&self.journal_path)?;
        let (base, label) = match &self.data_path {
            Some(path) if path.exists() => load_dataset_file(path)?,
            _ => load_initial_data()?,
        };
        match Editor::recover_with_report(base, &frames) {
            Ok((editor, report)) => {
                let recovered_path = data_dir().join("neo-recovered.osm");
                if let Err(err) = write_osm_xml_file(editor.data(), &recovered_path, XmlWriteMode::Josm) {
                    self.status = tf("app.error.recover", &[("err", &err.to_string())]);
                    return Ok(());
                }
                self.data_path = Some(recovered_path);
                self.set_editor(editor, format!("{label} (recovered)"));
                self.reopen_journal();
                self.status = tf(
                    "app.status.recovered_report",
                    &[
                        ("frames", &report.frames.to_string()),
                        ("applied", &report.applied.to_string()),
                        ("skipped", &report.skipped.to_string()),
                    ],
                );
            }
            Err(err) => self.status = tf("app.error.recover", &[("err", &err.to_string())]),
        }
        Ok(())
    }
}
