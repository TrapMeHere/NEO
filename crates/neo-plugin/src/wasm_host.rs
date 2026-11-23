//! wasmtime 50 宿主。插件是 core module，导入 `neo` 里的数字 ABI（WIT host-api 的数值镜像）。
//!
//! 不链接 WASI。fuel 杀掉死循环，`StoreLimits` 挡住线性内存扩张，一个实例 trap 不会关掉其他实例。

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use wasmtime::{Caller, Engine, Extern, Instance, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};

use crate::PluginError;
use crate::capability::CapabilitySet;
use crate::gate::{HostGate, PanelDescriptor, PendingCommand, host_code};
use crate::guests::builtin_plugins;
use crate::manifest::parse_manifest;
use crate::package::content_signature;

/// 宿主配置。
#[derive(Clone, Debug)]
pub struct PluginHostConfig {
    /// 每次 `on_load` / `on_event` 的 fuel。
    pub fuel_per_event: u64,
    /// 线性内存上限（字节）。
    pub memory_limit_bytes: usize,
    /// 每个事件最多入队多少条命令。
    pub commands_per_event: u32,
    /// 允许未签名插件。界面必须显示风险标记（[`LoadedPlugin::unsigned_dev`]）。
    pub developer_mode: bool,
}

impl Default for PluginHostConfig {
    fn default() -> Self {
        Self {
            fuel_per_event: 100_000_000,
            memory_limit_bytes: 64 * 1024 * 1024,
            commands_per_event: 20,
            developer_mode: false,
        }
    }
}

struct HostState {
    gate: HostGate,
    limits: StoreLimits,
}

struct LoadedPlugin {
    id: String,
    name: String,
    disabled: bool,
    unsigned_dev: bool,
    store: Store<HostState>,
    instance: Instance,
}

/// 已加载插件的公开摘要。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedPluginInfo {
    /// 插件 id。
    pub id: String,
    /// 显示名。
    pub name: String,
    /// 开发者模式里的未签名包。
    pub unsigned_dev: bool,
    /// 已因 trap 停用。
    pub disabled: bool,
}

/// 一次派发的结果。某个插件 trap 不会让函数本身失败。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DispatchReport {
    /// 正常返回的插件。
    pub ran: Vec<String>,
    /// trap 的插件和原因。
    pub trapped: Vec<(String, String)>,
    /// 未订阅该主题或已停用。
    pub skipped: Vec<String>,
}

/// 插件宿主。
pub struct PluginHost {
    engine: Engine,
    linker: Linker<HostState>,
    cfg: PluginHostConfig,
    http_hits: Arc<AtomicU32>,
    plugins: Vec<LoadedPlugin>,
}

impl PluginHost {
    /// 创建引擎。fuel 与 epoch 都打开；本实现用 fuel 做确定性的超时。
    pub fn new(cfg: PluginHostConfig) -> Result<Self, PluginError> {
        let mut config = wasmtime::Config::new();
        config.consume_fuel(true);
        config.epoch_interruption(true);
        // 关闭多内存：StoreLimits 的 memory_size 是"每个内存"的限额，允许
        // 多内存会让插件用 N 个 64MiB 内存绕过总内存上限。
        config.wasm_multi_memory(false);
        let engine = Engine::new(&config).map_err(|err| PluginError::Trapped(err.to_string()))?;
        let mut linker = Linker::new(&engine);
        link_host(&mut linker)?;
        Ok(Self {
            engine,
            linker,
            cfg,
            http_hits: Arc::new(AtomicU32::new(0)),
            plugins: Vec::new(),
        })
    }

    /// 编译并加载 WAT/WASM 文本。`granted` 是用户点过的授权，会和清单求交。
    pub fn load_wat(
        &mut self,
        manifest_toml: &str,
        wat: &str,
        granted: &CapabilitySet,
    ) -> Result<String, PluginError> {
        self.load_entry(manifest_toml, wat.as_bytes(), granted)
    }

    /// 编译并加载 `.wasm`/`.wat` 字节（`.neopkg` 入口使用）。
    pub fn load_binary(
        &mut self,
        manifest_toml: &str,
        entry: &[u8],
        granted: &CapabilitySet,
    ) -> Result<String, PluginError> {
        self.load_entry(manifest_toml, entry, granted)
    }

    fn load_entry(
        &mut self,
        manifest_toml: &str,
        entry: &[u8],
        granted: &CapabilitySet,
    ) -> Result<String, PluginError> {
        let manifest = parse_manifest(manifest_toml)?;
        if !manifest.api_compatible() {
            return Err(PluginError::ApiIncompatible {
                plugin: manifest.api.clone(),
                host: "1.x".into(),
            });
        }
        let unsigned_dev = if manifest.signature.is_empty() {
            if !self.cfg.developer_mode {
                return Err(PluginError::SignatureInvalid);
            }
            true
        } else {
            let expect = content_signature(manifest_toml, entry);
            if !expect.eq_ignore_ascii_case(&manifest.signature) {
                return Err(PluginError::SignatureInvalid);
            }
            false
        };
        let caps = granted.intersect(&manifest.capabilities);
        let module = Module::new(&self.engine, entry).map_err(|err| PluginError::Trapped(err.to_string()))?;
        let limits = StoreLimitsBuilder::new()
            .memory_size(self.cfg.memory_limit_bytes)
            .memories(1)
            .tables(2)
            .instances(1)
            .trap_on_grow_failure(false)
            .build();
        let gate = HostGate::new(manifest.id.clone(), caps, Arc::clone(&self.http_hits));
        let mut store = Store::new(&self.engine, HostState { gate, limits });
        store
            .set_fuel(self.cfg.fuel_per_event)
            .map_err(|err| PluginError::Trapped(err.to_string()))?;
        store.set_epoch_deadline(1_000_000);
        store.limiter(|state| &mut state.limits);
        store.data_mut().gate.command_budget = self.cfg.commands_per_event;
        let instance = self
            .linker
            .instantiate(&mut store, &module)
            .map_err(|err| PluginError::Trapped(err.to_string()))?;
        let on_load = instance
            .get_typed_func::<(), i32>(&mut store, "on_load")
            .map_err(|err| PluginError::Trapped(err.to_string()))?;
        on_load
            .call(&mut store, ())
            .map_err(|err| PluginError::Trapped(err.to_string()))?;
        let id = manifest.id.clone();
        self.plugins.push(LoadedPlugin {
            id: id.clone(),
            name: manifest.name,
            disabled: false,
            unsigned_dev,
            store,
            instance,
        });
        Ok(id)
    }

    /// 向所有订阅了 `topic` 的插件派发。一个插件 trap 只停用它自己。
    pub fn dispatch(&mut self, topic: &str, _payload_json: &str) -> DispatchReport {
        let mut report = DispatchReport::default();
        let fuel = self.cfg.fuel_per_event;
        let budget = self.cfg.commands_per_event;
        for plugin in &mut self.plugins {
            if plugin.disabled {
                report.skipped.push(plugin.id.clone());
                continue;
            }
            if !plugin.store.data().gate.caps.allows_event(topic) {
                report.skipped.push(plugin.id.clone());
                continue;
            }
            plugin.store.data_mut().gate.command_budget = budget;
            if plugin.store.set_fuel(fuel).is_err() {
                plugin.disabled = true;
                report
                    .trapped
                    .push((plugin.id.clone(), "fuel reset failed".into()));
                continue;
            }
            plugin.store.set_epoch_deadline(1_000_000);
            let bytes = topic.as_bytes();
            let len = bytes.len().min(96);
            let Some(memory) = plugin.instance.get_memory(&mut plugin.store, "memory") else {
                plugin.disabled = true;
                report
                    .trapped
                    .push((plugin.id.clone(), "memory export missing".into()));
                continue;
            };
            if memory.write(&mut plugin.store, 128, &bytes[..len]).is_err() {
                plugin.disabled = true;
                report
                    .trapped
                    .push((plugin.id.clone(), "topic write failed".into()));
                continue;
            }
            let Ok(on_event) = plugin
                .instance
                .get_typed_func::<(i32, i32), i32>(&mut plugin.store, "on_event")
            else {
                plugin.disabled = true;
                report
                    .trapped
                    .push((plugin.id.clone(), "on_event missing".into()));
                continue;
            };
            match on_event.call(&mut plugin.store, (128, len as i32)) {
                Ok(_) => report.ran.push(plugin.id.clone()),
                Err(err) => {
                    plugin.disabled = true;
                    report.trapped.push((plugin.id.clone(), err.to_string()));
                }
            }
        }
        report
    }

    /// 取走全部已入队命令。
    pub fn drain_commands(&mut self) -> Vec<PendingCommand> {
        let mut out = Vec::new();
        for plugin in &mut self.plugins {
            out.append(&mut plugin.store.data_mut().gate.commands);
        }
        out
    }

    /// 全部面板。
    pub fn panels(&self) -> Vec<PanelDescriptor> {
        self.plugins
            .iter()
            .flat_map(|plugin| plugin.store.data().gate.panels.clone())
            .collect()
    }

    /// 已加载插件。
    pub fn plugins(&self) -> Vec<LoadedPluginInfo> {
        self.plugins
            .iter()
            .map(|plugin| LoadedPluginInfo {
                id: plugin.id.clone(),
                name: plugin.name.clone(),
                unsigned_dev: plugin.unsigned_dev,
                disabled: plugin.disabled,
            })
            .collect()
    }

    /// 某个插件的线性内存字节数。
    pub fn memory_size(&mut self, id: &str) -> Option<usize> {
        let plugin = self.plugins.iter_mut().find(|plugin| plugin.id == id)?;
        let memory = plugin.instance.get_memory(&mut plugin.store, "memory")?;
        Some(memory.data_size(&plugin.store))
    }

    /// 放行的 HTTP 次数。被拒绝的请求不加。
    pub fn http_hits(&self) -> u32 {
        self.http_hits.load(Ordering::Relaxed)
    }

    /// 全部放行的 URL。
    pub fn http_log(&self) -> Vec<String> {
        self.plugins
            .iter()
            .flat_map(|plugin| plugin.store.data().gate.http_log.clone())
            .collect()
    }
}

fn link_host(linker: &mut Linker<HostState>) -> Result<(), PluginError> {
    linker
        .func_wrap(
            "neo",
            "snapshot_version",
            |caller: Caller<'_, HostState>| -> i64 { caller.data().gate.snapshot_version as i64 },
        )
        .map_err(|err| PluginError::Trapped(err.to_string()))?;
    linker
        .func_wrap(
            "neo",
            "submit_command",
            |mut caller: Caller<'_, HostState>, np: i32, nl: i32, ap: i32, al: i32| -> i32 {
                let name = match read_guest(&mut caller, np, nl) {
                    Ok(value) => value,
                    Err(code) => return code,
                };
                let args = match read_guest(&mut caller, ap, al) {
                    Ok(value) => value,
                    Err(code) => return code,
                };
                match caller.data_mut().gate.submit_command(&name, &args) {
                    Ok(_) => 0,
                    Err(err) => host_code(err),
                }
            },
        )
        .map_err(|err| PluginError::Trapped(err.to_string()))?;
    linker
        .func_wrap(
            "neo",
            "register_panel",
            |mut caller: Caller<'_, HostState>, ip: i32, il: i32, tp: i32, tl: i32| -> i32 {
                let id = match read_guest(&mut caller, ip, il) {
                    Ok(value) => value,
                    Err(code) => return code,
                };
                let title = match read_guest(&mut caller, tp, tl) {
                    Ok(value) => value,
                    Err(code) => return code,
                };
                match caller.data_mut().gate.register_panel(&id, &title) {
                    Ok(panel) => panel as i32,
                    Err(err) => -host_code(err),
                }
            },
        )
        .map_err(|err| PluginError::Trapped(err.to_string()))?;
    linker
        .func_wrap(
            "neo",
            "set_panel_status",
            |mut caller: Caller<'_, HostState>, panel: i32, ptr: i32, len: i32| -> i32 {
                let text = match read_guest(&mut caller, ptr, len) {
                    Ok(value) => value,
                    Err(code) => return code,
                };
                match caller.data_mut().gate.set_panel_status(panel as u32, &text) {
                    Ok(()) => 0,
                    Err(err) => host_code(err),
                }
            },
        )
        .map_err(|err| PluginError::Trapped(err.to_string()))?;
    linker
        .func_wrap(
            "neo",
            "read_file",
            |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| -> i32 {
                let path = match read_guest(&mut caller, ptr, len) {
                    Ok(value) => value,
                    Err(code) => return code,
                };
                match caller.data_mut().gate.read_file(&path) {
                    Ok(_) => 0,
                    Err(err) => host_code(err),
                }
            },
        )
        .map_err(|err| PluginError::Trapped(err.to_string()))?;
    linker
        .func_wrap(
            "neo",
            "http_request",
            |mut caller: Caller<'_, HostState>, up: i32, ul: i32, mp: i32, ml: i32| -> i32 {
                let url = match read_guest(&mut caller, up, ul) {
                    Ok(value) => value,
                    Err(code) => return code,
                };
                let method = match read_guest(&mut caller, mp, ml) {
                    Ok(value) => value,
                    Err(code) => return code,
                };
                match caller.data_mut().gate.http_request(&url, &method) {
                    Ok(_) => 0,
                    Err(err) => host_code(err),
                }
            },
        )
        .map_err(|err| PluginError::Trapped(err.to_string()))?;
    linker
        .func_wrap(
            "neo",
            "log",
            |mut caller: Caller<'_, HostState>, _level: i32, ptr: i32, len: i32| {
                let _ = read_guest(&mut caller, ptr, len);
            },
        )
        .map_err(|err| PluginError::Trapped(err.to_string()))?;
    Ok(())
}

fn read_guest(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> Result<String, i32> {
    if ptr < 0 || !(0..=65_536).contains(&len) {
        return Err(3);
    }
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return Err(3);
    };
    let mut buf = vec![0u8; len as usize];
    if memory.read(caller, ptr as usize, &mut buf).is_err() {
        return Err(3);
    }
    String::from_utf8(buf).map_err(|_| 3)
}

/// 跑三个内置插件并返回人读得懂的结果。CLI 用。
pub fn run_builtin_demo() -> Result<String, PluginError> {
    let cfg = PluginHostConfig {
        developer_mode: true,
        ..PluginHostConfig::default()
    };
    let mut host = PluginHost::new(cfg)?;
    let granted = CapabilitySet::grant_all(&[]);
    for (_id, manifest, wat) in builtin_plugins() {
        host.load_wat(&manifest, &wat, &granted)?;
    }
    host.dispatch("selection.changed", "{}");
    host.dispatch("utils.add-node", "{}");
    host.dispatch("buildings.rect", "{}");
    let mut lines = Vec::new();
    for plugin in host.plugins() {
        lines.push(format!(
            "plugin {} disabled={} unsigned={}",
            plugin.id, plugin.disabled, plugin.unsigned_dev
        ));
    }
    for panel in host.panels() {
        lines.push(format!(
            "panel {} {} status={}",
            panel.id, panel.title, panel.status
        ));
    }
    for cmd in host.drain_commands() {
        lines.push(format!("command {} {} {}", cmd.plugin, cmd.name, cmd.args_json));
    }
    Ok(lines.join("\n"))
}
