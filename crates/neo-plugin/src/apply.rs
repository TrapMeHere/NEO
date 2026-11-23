//! 把插件命令翻译成 `neo-editor` 事务。插件没有第二条写路径。

use neo_core::{LatLon, ObjectKind};
use neo_editor::{AddNode, AddWay, Editor};

use crate::PluginError;
use crate::gate::PendingCommand;

/// 执行一条已通过权限检查的命令。
pub fn apply_command(editor: &mut Editor, cmd: &PendingCommand) -> Result<(), PluginError> {
    let value: serde_json::Value = serde_json::from_str(&cmd.args_json)
        .map_err(|err| PluginError::Manifest(format!("command json: {err}")))?;
    match cmd.name.as_str() {
        "add-node" => {
            let (lat, lon) = lat_lon(&value)?;
            let mut cmd = AddNode::new(LatLon::from_degrees(lat, lon), Vec::new());
            cmd.id = Some(editor.next_local_id());
            editor
                .execute(Box::new(cmd))
                .map_err(|err| PluginError::Command(err.to_string()))?;
            Ok(())
        }
        "add-way" => {
            let nodes = value
                .get("nodes")
                .and_then(|v| v.as_array())
                .ok_or_else(|| PluginError::Manifest("add-way requires nodes".into()))?;
            let mut ids = Vec::with_capacity(nodes.len());
            for node in nodes {
                let pair = node
                    .as_array()
                    .ok_or_else(|| PluginError::Manifest("node must be [lat, lon]".into()))?;
                let lat = pair
                    .first()
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| PluginError::Manifest("lat".into()))?;
                let lon = pair
                    .get(1)
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| PluginError::Manifest("lon".into()))?;
                let id = editor.next_local_id();
                let mut cmd = AddNode::new(LatLon::from_degrees(lat, lon), Vec::new());
                cmd.id = Some(id);
                editor
                    .execute(Box::new(cmd))
                    .map_err(|err| PluginError::Command(err.to_string()))?;
                ids.push(id);
            }
            if ids.len() < 2 {
                return Err(PluginError::Manifest("add-way needs at least two nodes".into()));
            }
            let tags = tags_of(value.get("tags"));
            let mut cmd = AddWay::new(ids, tags);
            cmd.id = Some(editor.next_local_id());
            editor
                .execute(Box::new(cmd))
                .map_err(|err| PluginError::Command(err.to_string()))?;
            Ok(())
        }
        other => Err(PluginError::Manifest(format!("unknown plugin command {other}"))),
    }
}

fn lat_lon(value: &serde_json::Value) -> Result<(f64, f64), PluginError> {
    let lat = value
        .get("lat")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| PluginError::Manifest("lat".into()))?;
    let lon = value
        .get("lon")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| PluginError::Manifest("lon".into()))?;
    // NaN 会被 `as i32` 静默变成 0（几内亚湾），超范围值会被饱和；
    // 插件是外部输入，必须在进入 DataSet 前拒绝。
    neo_core::LatLon::try_from_degrees(lat, lon)
        .ok_or_else(|| PluginError::Manifest(format!("invalid coordinates {lat},{lon}")))?;
    Ok((lat, lon))
}

fn tags_of(value: Option<&serde_json::Value>) -> Vec<(String, String)> {
    let Some(obj) = value.and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    obj.iter()
        .filter_map(|(key, val)| val.as_str().map(|text| (key.clone(), text.to_owned())))
        .collect()
}

/// 读出 way 的标签文本，测试和 CLI 用。
pub fn way_tags(editor: &Editor) -> Vec<(String, String)> {
    editor
        .data()
        .iter_ways()
        .flat_map(|way| {
            editor.data().tag_pairs(way.tags).iter().map(|(key, value)| {
                (
                    editor.data().resolve(*key).to_owned(),
                    editor.data().resolve(*value).to_owned(),
                )
            })
        })
        .collect()
}

/// 数据集里可见 way 的种类计数，避免测试依赖私有字段。
pub fn object_kind_name(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Node => "node",
        ObjectKind::Way => "way",
        ObjectKind::Relation => "relation",
        ObjectKind::Changeset => "changeset",
    }
}
