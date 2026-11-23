# 15 · 插件系统（Plugin System）

本篇定义 `neo-plugin`：以 **WASM Component Model**（wasmtime 50 + wit-bindgen 0.63）承载第三方扩展，插件只能"读快照、交命令、收事件、注册面板"，永不直接触碰 `neo-store`。核心决策是**放弃 Rust 动态库 ABI**——Rust 没有稳定 ABI，`abi_stable 0.11.3` 只能缓解、不能消除工具链与依赖版本耦合；WASM 用一层显式 WIT 契约把"兼容性"变成可版本化、可测试的工程问题。

## 1 目标与范围

| 编号 | 目标 | 判定 |
|---|---|---|
| G1 | 第三方不重编译 NEO 即可扩展 | `.wasm` 跨 NEO minor 版本可用 |
| G2 | 恶意/有缺陷插件不能破坏数据或拖垮 UI | 越权报错、超预算被杀、主进程不崩 |
| G3 | 能力可审计、可最小化授权 | manifest 声明 + 运行时双重检查 |
| G4 | JOSM 插件作者有可执行迁移路径 | 能力映射表 + 等价实现示例 |
| G5 | 故障隔离到单插件 | 禁用/卸载粒度 = 单插件 |

**范围外**：JVM/Java 二进制兼容（不做、不承诺）、插件市场后端、进程外隔离（v1 为 in-process wasmtime，高危插件可选子进程模式）、插件 GPU 直通。
**依赖**：`neo-plugin → neo-core, neo-editor`，feature 门控 `wasm-plugins`（默认关；关闭时只剩 manifest 解析与类型定义，不引入 wasmtime）。只读地图查询需要一层 trait 缝（见 §2.7，本文指出的架构缺口）。

## 2 设计

### 2.1 为什么不是 Rust 动态库

| 方案 | 兼容性 | 安全性 | 分发 | 结论 |
|---|---|---|---|---|
| `cdylib` + 手写 `extern "C"` | 每个类型都要约定 `Vec`/`String` 生命周期；跨边界即 UB 风险 | 进程内任意代码，无沙箱 | 每平台每架构一份 | ❌ |
| `abi_stable 0.11.3` | `StableAbi` 稳定布局，但**仍绑定 rustc 版本与依赖 Major 版本** | 无沙箱 | 必须与宿主同编译器构建 | ❌ |
| WASM core module | 稳定、跨语言 | 沙箱 | 单文件 | ⚠️ 只能传数字/线性内存，需手写 ABI |
| **WASM Component** | WIT 接口 + 语义化版本，类型安全 | 沙箱 + 能力模型 | 单文件 | ✅ |

`abi_stable` 的失败模式值得记录：它保证"布局稳定"，不保证"依赖单例唯一"。插件与宿主各自静态链接 `tracing` 时两套 subscriber 互相覆盖；链接 `wgpu` 时两个实例争抢同一 `Instance`。WASM 把插件放进独立线性内存与独立实例，这类问题从"偶发崩溃"变成"编译期不允许"。

### 2.2 WIT 契约与版本化

宿主与插件之间只有一份契约：`wit/neo-plugin.wit`（`wit-bindgen 0.63` 生成两侧绑定），按 `package neo:plugin@1.0.0` 版本化：

| 变更 | 版本位 | 宿主行为 |
|---|---|---|
| 新增可选导出/事件主题/capability | minor | 兼容，继续加载 |
| 新增必填导入、改字段类型或语义、删函数 | major | 拒绝加载旧插件并提示"需要更新" |
| 仅注释/文档 | patch | 无影响 |

```wit
package neo:plugin@1.0.0;

interface types {
  enum object-kind { node, way, relation }
  record object-ref { kind: object-kind, id: u64 }
  record bbox { min-lon: s32, min-lat: s32, max-lon: s32, max-lat: s32 }  // 1e-7 度
  record tag { key: string, value: string }
  record object-view { ref: object-ref, version: u32, deleted: bool, tags: list<tag>,
    lat: option<s32>, lon: option<s32>, members: list<object-ref>, member-roles: list<string> }
  enum log-level { trace, debug, info, warn, error }
  variant host-error { permission-denied(string), not-found(object-ref),
    invalid-argument(string), internal(string), quota-exceeded(string) }
}

interface host-api {
  use types.{object-ref, object-view, bbox, log-level, host-error};
  snapshot-version: func() -> u64;                                   // 只读查询面
  lookup: func(refs: list<object-ref>) -> list<option<object-view>>;
  query-bbox: func(area: bbox, tag-filter: option<string>, limit: u32) -> list<object-ref>;
  parents-of: func(r: object-ref) -> list<object-ref>;
  submit-command: func(name: string, args-json: string) -> result<u64, host-error>;  // 唯一写路径
  subscribe: func(topics: list<string>) -> result<u64, host-error>;
  register-panel: func(id: string, title: string, preferred-width: u32) -> result<u32, host-error>;
  set-panel-status: func(panel: u32, text: string) -> result<_, host-error>;
  http-request: func(url: string, method: string, body: option<list<u8>>) -> result<list<u8>, host-error>;
  read-file: func(relative-path: string) -> result<list<u8>, host-error>;
  log: func(level: log-level, message: string);
}

world neo-plugin {
  import host-api;
  export on-load: func(manifest-json: string) -> result<_, string>;
  export on-event: func(topic: string, payload-json: string) -> result<_, string>;
  export render-panel: func(panel: u32, width: u32, height: u32) -> result<list<u8>, string>;
  export on-unload: func();
}
```

`render-panel` 返回**声明式绘制列表**（文本/按钮/表/滑块），由宿主用 egui 渲染。插件拿不到 GPU 句柄、拿不到 `egui::Context`，因此不可能插入任意绘制或阻塞帧。

### 2.3 能力模型与权限清单

| 能力 | 允许的 WIT 调用 | 默认 | 说明 |
|---|---|---|---|
| `read` | `snapshot-version`/`lookup`/`query-bbox`/`parents-of` | ✅ 隐式 | 只读，隐私低风险 |
| `command` | `submit-command` | ❌ | 命令仍走 `neo-editor` 校验 |
| `events` | `subscribe` + `on-event` | ❌ | 主题白名单 |
| `ui` | `register-panel`/`set-panel-status`/`render-panel` | ❌ | 面板数上限 8 |
| `network` | `http-request` | ❌ | 域名白名单 + 仅 HTTPS + 禁自定义头（防令牌外泄） |
| `filesystem` | `read-file` | ❌ | 仅插件自身目录、只读、禁 `..` |
| `imagery` | 经 `command` 间接添加影像层 | ❌ | 单列便于审计 |

**双重检查**：manifest 的 `[capabilities]` 决定"安装时展示给用户的权限清单"；运行时 `CapabilitySet` 决定"实际放行"。两者求交——即使声明了 `network`，用户未授予仍返回 `permission-denied`。这防止"安装即全权"。

### 2.4 沙箱资源限制

| 维度 | 机制（wasmtime 50） | 默认 | 失败表现 |
|---|---|---|---|
| CPU | `Config::consume_fuel(true)` + `Store::set_fuel` | 1e8 fuel/事件 | `Trap::OutOfFuel` ⇒ 记 `quota-exceeded`，插件禁用而非反复重试 |
| 墙钟 | `Config::epoch_interruption(true)` + `Store::set_epoch_deadline(1)` + 线程 `Engine::increment_epoch()` | epoch 1ms、deadline 100ms | 超时 trap |
| 内存 | `ResourceLimiter` / `StoreLimitsBuilder` | 64 MiB 线性内存、表 10k | 分配失败 ⇒ trap，插件禁用 |
| 文件/网络 | 不链接 WASI p2 的 `filesystem`/`sockets` | — | 只能走受控 host 函数 |
| 重入 | 单插件单实例，事件串行 | — | 插件内不可递归调宿主命令 |
| 编译 | `Engine::precompile_component` + `Config::cache` | 缓存到 `<cache>/wasm/` | 冷启动成本【待实测，见 docs/14-performance-and-verification.md】 |

`fuel` 与 `epoch` 必须**同时**开：fuel 防死循环（确定性、可计费），epoch 防"合法但长"的调用（如 10 万次 `lookup`）。

### 2.5 打包、签名与分发

```text
cool-plugin-1.2.0.neopkg  (= zip)
├── manifest.toml          # 权限清单，安装时展示
├── component.wasm         # 可选：precompile 后的组件
├── assets/icon.png
└── signature.ed25519      # 对 manifest + component 的 SHA-256 做 detached 签名
```

```toml
schema = 1
id = "org.example.coolplugin"
name = "Cool Plugin"
version = "1.2.0"
api = "1.0"                        # 宿主 WIT 版本，语义化兼容区间 1.x
min_neo_version = "0.1.0"
entry = "component.wasm"

[capabilities]
command = true
ui = true
events = ["edit.committed", "snapshot.published"]
network = ["https://api.example.org"]
filesystem = false
imagery = false
```

校验顺序：zip 完整性（列表 + 无路径穿越）→ 签名 → manifest 反序列化与字段校验 → `api` 兼容 → `Component::from_file` → 能力协商 → `on-load`。未签名插件仅"开发者模式"可加载，UI 必须持续显示风险标记；正式分发要求签名（验签库选型见 §2.7）。

### 2.6 宿主生命周期与事件

```text
PluginHost::load_all()
 ├─ 扫描 <config>/plugins/*.neopkg → 校验 + 解析 manifest
 │    （失败 ⇒ LoadReport::Rejected，不影响其他插件）
 ├─ Engine::new(Config{consume_fuel, epoch_interruption, cache})
 ├─ Component::from_file → Linker::instantiate（注入 HostApiImpl 的 StoreData）
 └─ on-load(manifest_json) → 记录 topics 订阅
事件循环（UI 帧边界，非帧内）
 ├─ 收集本帧事件，按 topic 去重合并（同 topic 只派发最新状态，防事件风暴）
 ├─ set_fuel + set_epoch_deadline → on-event(topic, json)
 │    └─ 插件只能 submit-command（入队，真正提交发生在下一次编辑管线）
 └─ 面板：render-panel → 声明式列表 → egui 绘制
```

| topic | 载荷要点 | 典型用途 |
|---|---|---|
| `project.*` / `layer.*`（opened、closed、added、removed、visibility） | project/layer id, kind | 初始化、图层工具 |
| `selection.changed` | refs[]（>1000 只给 count） | 上下文面板 |
| `edit.committed` / `edit.undone` / `edit.redone` | txn id, version, changed refs[] | 自动校验、撤销感知 |
| `snapshot.published` | snapshot version | 增量重算 |
| `viewport.changed` | bbox, zoom | 懒加载 |
| `upload.started` / `upload.finished` / `upload.failed` | changeset id, counts, conflict summary | 上传后动作 |
| `tool.activated` / `tool.deactivated` | tool id | 工具协作 |

事件是**只读通知**：在 `on-event` 中调 `submit-command` 允许，但它只是入队，`EditCommand` 的校验、journal、undo 全部照常生效——插件不享有任何绕过编辑器的特权。

### 2.7 版本兼容与架构缺口

宿主侧 `HostApi` 的实现需要三样东西：**当前只读快照**、**命令提交入口**、**面板注册表**。前两者的能力在 `neo-store` 与 `neo-editor`，而 `neo-plugin` 只允许依赖 `neo-core, neo-editor`。可选解法：

| 方案 | 说明 | 取舍 |
|---|---|---|
| A. `MapQuery` trait 下沉 `neo-core` | `neo-store` 实现、`neo-plugin` 只依赖 trait | ✅ 成本最低，但查询语义下沉到叶子 crate，需 02 白名单与概念归属评审 |
| B. 新增 `neo-query` 叶子 crate（trait + DTO） | 概念更干净 | ✅ 多一个 crate 与一条依赖边 |
| C. `neo-editor` 再导出 `neo-store` 类型 | 最省事 | ❌ 事实让 `neo-plugin` 传递依赖 `neo-store`，破坏依赖可审计性 |

**本文建议 A**，并把"`neo-plugin` 缺少只读查询缝"记为架构缺口：当前 workspace 中没有任何 `neo-plugin` 可依赖的 crate 拥有空间查询与交叉引用能力，这条缝**尚未存在**，实现前必须补齐，否则 `query-bbox` 无法实现，插件只能看到 `neo-editor` 已暴露的选择集。签名验签库同样未在依赖清单内，需单列选型（Ed25519）。

### 2.8 JOSM 插件迁移：等价能力映射

JOSM 插件是 Java 字节码，直接访问 `org.openstreetmap.josm.*` 内部 API（`MapView`/`DataSet` 被大量直接引用），**无法直接移植**。可行路径是"能力重述"：

| JOSM 能力（典型插件） | JOSM 形态 | NEO 等价物 | 成本 |
|---|---|---|---|
| 输入模式（绘制/拖拽） | 继承 `MapMode` | `neo-tools` 的 `Tool` + `command` 提交编辑 | 中：交互需重写 |
| 数据图层/自定义绘制 | `Layer` + `PaintVisitor` | ❌ 不允许自定义渲染；提供只读"数据源层"由宿主绘制 | 高：能力有意裁剪 |
| 校验规则 | `Validator` 测试集 | `neo-validate` 声明式规则（插件描述，宿主执行） | 低–中 |
| 上传挂钩 | `UploadHook` | `upload.*` 事件 + `command` | 低 |
| 影像源 / 远程 API | `ImageryInfo` 提供者 / `HttpURLConnection` | `imagery` 能力 + 配置项 / `network`（域名白名单） | 低 |
| 首选项面板 | `PreferenceSetting` | `ui` + 声明式面板 | 中 |
| 脚本（JSR-223/GraalJS） | 运行时执行 JS | v1 不内嵌 JS 引擎；收敛为小型 WASM 插件 | 高 |

**关于脚本插件**：JOSM 生态有一类插件本质是"JS/Groovy 小逻辑"。NEO v1 不内嵌 JS 引擎（依赖清单无此组件，引入 QuickJS/GraalJS 会带来新沙箱面与体积）。替代路径两条，均需显式立项：(1) 声明式规则——把"条件 → 动作"收敛进 `neo-validate` 规则 DSL，覆盖多数 tag 检查类脚本；(2) WIT 生成模板——`cargo generate` 模板把 20 行 JS 变成 60 行 Rust/WASM，成本可控且获得类型安全。若要支持 GraalJS 兼容层，需评估在 wasmtime 上运行 QuickJS-WASM 的预算，当前不做承诺。

## 3 关键接口

```rust
// neo-plugin/src/lib.rs
pub struct PluginHostConfig { pub plugin_dir: PathBuf, pub cache_dir: PathBuf,
    pub fuel_per_event: u64, pub epoch_deadline_ms: u64, pub memory_limit_bytes: usize,
    pub max_panels: usize, pub developer_mode: bool }

pub struct PluginHost { /* Engine + 每插件 Store/Instance + 权限表 */ }
impl PluginHost {
    pub fn new(cfg: PluginHostConfig) -> Result<Self, PluginError>;
    pub fn load_all(&mut self) -> Result<Vec<LoadReport>, PluginError>;
    pub fn load(&mut self, pkg: &Path, granted: CapabilitySet) -> Result<PluginId, PluginError>;
    pub fn dispatch(&mut self, events: &[HostEvent]) -> DispatchReport;
    pub fn drain_commands(&mut self) -> Vec<PendingCommand>;   // 交给编辑管线
    pub fn panels(&self) -> Vec<PanelDescriptor>;
    pub fn shutdown(self);
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PluginManifest { pub schema: u32, pub id: String, pub name: String, pub version: String,
    pub api: String, pub entry: String, pub min_neo_version: Option<String>,
    pub capabilities: ManifestCaps, pub limits: ManifestLimits }
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ManifestCaps { pub command: bool, pub ui: bool, pub events: Vec<String>,
    pub network: Vec<String>, pub filesystem: bool, pub imagery: bool }

#[derive(Debug, Clone, Default)]
pub struct CapabilitySet { /* bitflags 风格；manifest ∩ 用户授权 */ }
impl CapabilitySet {
    pub fn grants(&self, need: Capability) -> bool;
    pub fn intersect(self, declared: &ManifestCaps) -> Self;
}

/// 宿主的全部可见面：只读查询 + 命令提交 + UI 注册。
/// 实现放在 app 层（apps/neo-desktop），从而不破坏 neo-plugin 的依赖集合。
pub trait HostApi: Send {
    fn snapshot_version(&self) -> u64;
    fn lookup(&self, refs: &[ObjectRef]) -> Vec<Option<ObjectView>>;
    fn query_bbox(&self, area: Bbox, filter: Option<&str>, limit: u32) -> Vec<ObjectRef>;
    fn parents_of(&self, r: ObjectRef) -> Vec<ObjectRef>;
    fn submit_command(&mut self, name: &str, args_json: &str) -> Result<u64, HostError>;
    fn http_request(&mut self, url: &str, method: &str, body: Option<&[u8]>) -> Result<Vec<u8>, HostError>;
    fn read_file(&mut self, rel: &str) -> Result<Vec<u8>, HostError>;
    fn log(&mut self, level: LogLevel, msg: &str);
}

pub enum LoadReport {
    Loaded { id: PluginId, name: String, caps: CapabilitySet },
    Rejected { path: PathBuf, reason: RejectReason },
    Disabled { id: PluginId, reason: DisableReason },   // 预算超限后
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("manifest: {0}")] Manifest(String),
    #[error("incompatible api: plugin={plugin}, host={host}")] ApiIncompatible { plugin: String, host: String },
    #[error("signature invalid")] SignatureInvalid,
    #[error("capability denied: {0}")] CapabilityDenied(String),
    #[error("plugin trapped: {0}")] Trapped(String),
    #[error("quota exceeded: {0}")] QuotaExceeded(String),
}
```

## 4 数据流与时序

```text
UI      PluginHost         Manifest/签名     wasmtime Engine     HostApi(app 层)
 │ 安装    │                   │                 │                    │
 │────────▶│ open .neopkg → unzip + 校验 ──────▶│                    │
 │         │◀─ ok / SignatureInvalid             │                    │
 │         │── api 兼容（1.x）→ Component::from_file ──────────────▶│
 │         │── Linker::instantiate ─────────────▶│ 独立 Store/内存    │
 │         │── on-load(manifest_json) ──────────▶│ fuel/epoch 已设    │
 │◀ 权限清单│（manifest ∩ 用户授权）              │                    │
 │ 选择变化 │── dispatch([selection.changed]) ───▶│ on-event           │
 │         │                                    │─ lookup → 快照 ───▶│
 │         │                                    │─ submit-command ──▶│ 入队
 │         │◀── PendingCommand（下一帧提交）─────│                    │
 │         │── drain_commands → neo-editor 管线 ────────────────────▶│
 │         │── render-panel → 声明式列表 → egui 绘制                 │
```

预算超限：`on-event` 死循环 ⇒ fuel 耗尽 ⇒ `Trap::OutOfFuel` ⇒ 记 `quota-exceeded`；连续 3 次自动 `Disabled`（可手动重置）；该帧其他插件与其各自实例不受影响。

## 5 权衡与替代方案

| 决策 | 选择 | 替代 | 放弃原因 |
|---|---|---|---|
| 插件格式 | WASM Component | `cdylib`/`abi_stable` | 无稳定 ABI、依赖单例冲突、无沙箱 |
| 隔离级别 | 进程内 wasmtime | 子进程 + IPC | 每次查询要序列化往返，交互面板延迟不可接受；留作高危插件可选模式 |
| UI 扩展 | 声明式绘制列表 | 暴露 `egui::Ui` | 任意绘制可破坏布局/阻塞帧；声明式可限制、可审计 |
| 资源限制 | fuel + epoch + 内存上限 | 仅超时 / 仅 fuel | 仅超时不可计费；仅 fuel 挡不住"合法长调用" |
| 网络 | 域名白名单 + 禁自定义头 | 全放行 | 防插件窃取 OAuth 令牌（令牌只存在于宿主 HTTP 层） |
| 签名 | detached Ed25519 | 无签名 | 插件能提交编辑，供应链攻击面过大 |

## 6 风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| WIT 契约演进破坏旧插件 | 插件集体失效 | semver 规则 + 双版本 `Linker` 并行；CI 保留 v1 golden 插件 |
| fuel 预算过紧 | 正常插件被误杀 | 按 topic 分级 + 用户可调 + `Disabled` 可恢复 |
| 事件风暴 | 帧时间抖动 | 帧边界派发 + 同 topic 合并 + 每插件每帧一次 |
| 高频提交命令 | 编辑历史污染 | 命令速率限制（默认 20 次/秒）+ 全部可撤销 |
| wasm 冷启动 | 首次打开慢 | `precompile_component` + 编译缓存 + 懒实例化 |
| 签名密钥被盗 / 插件读到敏感数据 | 无法撤销 / 隐私泄露 | manifest `revoked_after` + 本地撤销列表；`read` 仅暴露快照内容，notes/GPX 层需额外能力 |

## 7 验收标准

- [ ] 加载 WIT `1.0` 示例插件：读快照、按 bbox 查询、提交一条 `AddNode`、渲染一个面板。
- [ ] 未声明 `command` 的插件调 `submit-command` 返回 `permission-denied`，且不产生任何编辑。
- [ ] 死循环插件在 100ms 内被 trap，宿主帧循环不阻塞，其他插件继续工作。
- [ ] 申请超 64MiB 内存被限流，宿主不 OOM；`read-file("../x")` 返回 `permission-denied`。
- [ ] 未签名插件在 `developer_mode=false` 下被拒；`true` 下加载但界面带风险标记。
- [ ] `api = "2.0"` 的插件被拒绝并给出可读错误。
- [ ] `wasm-plugins` 关闭时 `neo-plugin` 不引入 wasmtime（`cargo tree` 断言）；`cargo test -p neo-plugin` 与 `clippy -D warnings` 通过。

## 8 参考

- wasmtime 50 组件模型（`Component::from_file`、`Linker::instantiate`、`bindgen!`）：<https://docs.rs/wasmtime/50.0.1/wasmtime/component/>
- wasmtime 资源限制（`consume_fuel`、`epoch_interruption`、`increment_epoch`、`set_fuel`、`set_epoch_deadline`、`ResourceLimiter`）：<https://docs.rs/wasmtime/50.0.1/wasmtime/struct.Config.html>
- 预编译与编译缓存（`Engine::precompile_component`、`Config::cache`）：<https://docs.rs/wasmtime/50.0.1/wasmtime/struct.Engine.html#method.precompile_component>
- WIT 语言与包版本化：<https://component-model.bytecodealliance.org/design/wit.html>
- `wit-bindgen 0.63`：<https://docs.rs/wit-bindgen/0.63.0/wit_bindgen/>
- JOSM 插件清单字段（`Plugin-Mainversion`、`Plugin-Class`、`Plugin-Version`、`Plugin-Description`、`Plugin-Stage` 等）：<https://wiki.openstreetmap.org/wiki/JOSM/Plugins>
- 相关：[Workspace 与依赖规则](./02-workspace-and-dependency-rules.md) · [编辑核心与撤销](./06-editing-core-and-undo.md) · [系统架构](./01-system-architecture.md) · [ADR-0007](./adr/0007-wasm-plugins.md)
