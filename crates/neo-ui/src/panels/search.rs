//! search panel.

use egui_phosphor::regular as ph;
use neo_core::{ObjectId, ObjectKind};
use neo_i18n::{t, tf};
use neo_store::DataSet;

use super::UiActions;

// ---------------------------------------------------------------------------
// Settings window (language + shortcuts)
// ---------------------------------------------------------------------------

/// Settings window: language switch + shortcut overview.
/// Search result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    /// Object kind.
    pub kind: ObjectKind,
    /// Object ID.
    pub id: ObjectId,
    /// Display text (kind id name/key tag).
    pub label: String,
}

/// Search window state.
#[derive(Clone, Debug)]
pub struct SearchUiState {
    /// Query string. Supports `id:123`, `key=value`, and plain text.
    pub query: String,
    /// Whether to search nodes.
    pub nodes: bool,
    /// Whether to search ways.
    pub ways: bool,
    /// Whether to search relations.
    pub relations: bool,
    /// Key used for the last refresh (query + filters).
    last_key: String,
    /// Results (at most 500).
    hits: Vec<SearchHit>,
}

impl Default for SearchUiState {
    fn default() -> Self {
        Self {
            query: String::new(),
            nodes: true,
            ways: true,
            relations: true,
            last_key: String::new(),
            hits: Vec::new(),
        }
    }
}

/// Search window: search by ID / `key=value` / tag text; clicking a result selects and locates it.
pub fn search_window(
    ctx: &egui::Context,
    actions: &mut UiActions,
    open: &mut bool,
    state: &mut SearchUiState,
    data: &DataSet,
) {
    if !*open {
        return;
    }
    egui::Window::new(format!("{} {}", ph::MAGNIFYING_GLASS, t("search.title")))
        .open(open)
        .resizable(true)
        .default_width(460.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(t("search.query"));
                ui.add(egui::TextEdit::singleline(&mut state.query).desired_width(260.0));
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut state.nodes, t("search.nodes"));
                ui.checkbox(&mut state.ways, t("search.ways"));
                ui.checkbox(&mut state.relations, t("search.relations"));
            });
            let key = format!(
                "{}\u{1f}{}{}{}",
                state.query, state.nodes, state.ways, state.relations
            );
            if key != state.last_key {
                state.last_key = key;
                state.hits = search_hits(data, &state.query, state.nodes, state.ways, state.relations);
            }
            ui.label(tf("search.count", &[("n", &state.hits.len().to_string())]));
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("neo-search-results")
                .max_height(360.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for hit in &state.hits {
                        if ui.selectable_label(false, &hit.label).clicked() {
                            actions.select_objects = Some(vec![(hit.kind, hit.id)]);
                            actions.focus_object = Some((hit.kind, hit.id));
                        }
                    }
                });
        });
}

fn search_hits(data: &DataSet, query: &str, nodes: bool, ways: bool, relations: bool) -> Vec<SearchHit> {
    const LIMIT: usize = 500;
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let id_query = query
        .strip_prefix("id:")
        .and_then(|value| value.trim().parse::<i64>().ok());
    let tag_query = query
        .split_once('=')
        .map(|(key, value)| (key.trim().to_lowercase(), value.trim().to_lowercase()));
    let mut hits = Vec::new();
    let matches = |id: ObjectId, pairs: &[(neo_core::Sym, neo_core::Sym)]| -> bool {
        if let Some(wanted) = id_query {
            return id.0 == wanted;
        }
        if let Some((key, value)) = &tag_query {
            return pairs.iter().any(|(k, v)| {
                data.resolve(*k).eq_ignore_ascii_case(key) && data.resolve(*v).eq_ignore_ascii_case(value)
            });
        }
        if id.0.to_string().contains(&query) {
            return true;
        }
        pairs.iter().any(|(key, value)| {
            data.resolve(*key).to_lowercase().contains(&query)
                || data.resolve(*value).to_lowercase().contains(&query)
        })
    };
    let label = |kind: ObjectKind, id: ObjectId, pairs: &[(neo_core::Sym, neo_core::Sym)]| {
        let name = pairs
            .iter()
            .find(|(key, _)| data.resolve(*key) == "name")
            .map(|(_, value)| data.resolve(*value))
            .or_else(|| pairs.first().map(|(_, value)| data.resolve(*value)))
            .unwrap_or("");
        format!("{kind} {id}  {name}")
    };
    if nodes {
        for node in data.iter_nodes() {
            if hits.len() >= LIMIT {
                break;
            }
            if !node.meta.visible {
                continue;
            }
            let pairs = data.tag_pairs(node.tags);
            if matches(node.id, pairs) {
                hits.push(SearchHit {
                    kind: ObjectKind::Node,
                    id: node.id,
                    label: label(ObjectKind::Node, node.id, pairs),
                });
            }
        }
    }
    if ways {
        for way in data.iter_ways() {
            if hits.len() >= LIMIT {
                break;
            }
            if !way.meta.visible {
                continue;
            }
            let pairs = data.tag_pairs(way.tags);
            if matches(way.id, pairs) {
                hits.push(SearchHit {
                    kind: ObjectKind::Way,
                    id: way.id,
                    label: label(ObjectKind::Way, way.id, pairs),
                });
            }
        }
    }
    if relations {
        for relation in data.iter_relations() {
            if hits.len() >= LIMIT {
                break;
            }
            if !relation.meta.visible {
                continue;
            }
            let pairs = data.tag_pairs(relation.tags);
            if matches(relation.id, pairs) {
                hits.push(SearchHit {
                    kind: ObjectKind::Relation,
                    id: relation.id,
                    label: label(ObjectKind::Relation, relation.id, pairs),
                });
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use neo_core::{LatLon, Meta};
    use neo_osm::{Node, Tags};

    fn sample() -> DataSet {
        let mut data = DataSet::new();
        let key = data.intern("name");
        let value = data.intern("Main Street");
        data.push_node(Node {
            id: ObjectId(1),
            pos: LatLon::from_degrees(47.0, 8.0),
            tags: Tags::from_pairs([(key, value)]),
            meta: Meta::new_local(),
        })
        .unwrap();
        data
    }

    #[test]
    fn search_supports_id_tag_and_text() {
        let data = sample();
        assert_eq!(search_hits(&data, "id:1", true, true, true).len(), 1);
        assert_eq!(search_hits(&data, "name=Main Street", true, true, true).len(), 1);
        assert_eq!(search_hits(&data, "main", true, true, true).len(), 1);
        assert!(search_hits(&data, "id:2", true, true, true).is_empty());
        assert!(search_hits(&data, "main", false, false, false).is_empty());
    }
}
