//! 三个 JOSM 插件语义的 WASM 文本，外加沙箱探针。
//!
//! * `todo`：JOSM todo 的任务面板。加载时注册面板，选择变化时写状态。
//! * `utilsplugin`：utilsplugin2 的「加节点」。只在 `utils.add-node` 上提交 `add-node`。
//! * `buildings_tools`：矩形建筑。在 `buildings.rect` 上提交带 `building=yes` 的 `add-way`。
//!
//! 这些模块只导入 `neo` 命名空间，不链接 WASI。

/// Todo 插件（JOSM todo：选择变化 → 面板状态）。
pub fn todo_wat() -> String {
    base_module(
        r#"
        (data (i32.const 256) "todo")
        (data (i32.const 288) "Todo")
        (data (i32.const 320) "open tasks")
        (func (export "on_load") (result i32)
          (i32.store (i32.const 64)
            (call $panel (i32.const 256) (i32.const 4) (i32.const 288) (i32.const 4)))
          (drop (call $ver))
          (i32.const 0))
        (func (export "on_event") (param i32 i32) (result i32)
          (drop (call $status (i32.load (i32.const 64)) (i32.const 320) (i32.const 10)))
          (i32.const 0))
        "#,
    )
}

/// utilsplugin2 的加节点。
pub fn utils_wat() -> String {
    command_module("utils.add-node", "add-node", r#"{"lat":0,"lon":0}"#)
}

/// buildings_tools 的矩形建筑。
pub fn buildings_wat() -> String {
    command_module(
        "buildings.rect",
        "add-way",
        r#"{"tags":{"building":"yes"},"nodes":[[47,8],[47,8.001],[47.001,8.001],[47.001,8]]}"#,
    )
}

fn command_module(topic: &str, name: &str, args: &str) -> String {
    base_module(&format!(
        r#"
        (data (i32.const 400) "{topic}")
        (data (i32.const 480) "{name}")
        (data (i32.const 520) "{args}")
        (func $eq (param $a i32) (param $b i32) (param $n i32) (result i32)
          (local $i i32)
          (block $done
            (loop $loop
              (br_if $done (i32.eq (local.get $i) (local.get $n)))
              (if (i32.ne
                    (i32.load8_u (i32.add (local.get $a) (local.get $i)))
                    (i32.load8_u (i32.add (local.get $b) (local.get $i))))
                (then (return (i32.const 0))))
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (br $loop)))
          (i32.const 1))
        (func (export "on_load") (result i32) (i32.const 0))
        (func (export "on_event") (param $p i32) (param $n i32) (result i32)
          (if (i32.and
                (i32.eq (local.get $n) (i32.const {tlen}))
                (call $eq (local.get $p) (i32.const 400) (i32.const {tlen})))
            (then
              (drop (call $submit
                (i32.const 480) (i32.const {nlen})
                (i32.const 520) (i32.const {alen})))))
          (i32.const 0))
        "#,
        topic = wat_escape(topic),
        name = wat_escape(name),
        args = wat_escape(args),
        tlen = topic.len(),
        nlen = name.len(),
        alen = args.len(),
    ))
}

fn base_module(body: &str) -> String {
    format!(
        r#"(module
          (import "neo" "snapshot_version" (func $ver (result i64)))
          (import "neo" "submit_command" (func $submit (param i32 i32 i32 i32) (result i32)))
          (import "neo" "register_panel" (func $panel (param i32 i32 i32 i32) (result i32)))
          (import "neo" "set_panel_status" (func $status (param i32 i32 i32) (result i32)))
          (import "neo" "read_file" (func $read (param i32 i32) (result i32)))
          (import "neo" "http_request" (func $http (param i32 i32 i32 i32) (result i32)))
          (import "neo" "log" (func $log (param i32 i32 i32)))
          (memory (export "memory") 1)
          (func (export "on_unload"))
          {body}
        )"#
    )
}

fn wat_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// 三个真实插件的清单与 WAT。
pub fn builtin_plugins() -> Vec<(&'static str, String, String)> {
    vec![
        (
            "org.neo.todo",
            manifest("org.neo.todo", "Todo", false, true, &["selection.changed"]),
            todo_wat(),
        ),
        (
            "org.neo.utils",
            manifest("org.neo.utils", "Utils", true, false, &["utils.add-node"]),
            utils_wat(),
        ),
        (
            "org.neo.buildings",
            manifest("org.neo.buildings", "Buildings", true, false, &["buildings.rect"]),
            buildings_wat(),
        ),
    ]
}

fn manifest(id: &str, name: &str, command: bool, ui: bool, events: &[&str]) -> String {
    let events = events
        .iter()
        .map(|e| format!("\"{e}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"schema = 1
id = "{id}"
name = "{name}"
version = "1.0.0"
api = "1.0"
entry = "component.wat"
signature = ""
[capabilities]
command = {command}
ui = {ui}
events = [{events}]
network = []
filesystem = false
imagery = false
"#
    )
}
