# Gate-5 报告：完整生态

- **Gate id**：G5（docs/16 §2.6 / §2.8）
- **评估日期**：2026-11-16
- **代码状态**：工作区未提交，基线 commit `add2418`
- **本轮跑过的命令**：`cargo test -p neo-plugin --features wasm-plugins --offline`；`cargo test -p neo-cli --features plugins --offline`；`cargo check -p neo-desktop -p neo-ui --offline`；`neo-cli presets`；`neo-cli --features plugins plugins`；`neo-cli open --synth 1000000`

## 0 环境

| 项 | 值 |
|---|---|
| OS | Windows 11 10.0.26100 |
| Rust | 1.100.0 stable。workspace `rust-version` 仍是 1.95 |
| wasmtime | 50.0.1（cranelift 0.137.1）。这两个包声明 rustc ≥ 1.97 |
| 构建 | dev profile（optimized + debuginfo）。没有打 release 安装包 |
| feature | `wasm-plugins` 默认关闭。桌面二进制不链接 wasmtime |

默认构建不编译 wasmtime，所以 MSRV 1.95 的默认检查不受影响。打开 `wasm-plugins` / `neo-cli` 的 `plugins` 需要 rustc ≥ 1.97。workspace 的 `rust-version` 没有因此改成 1.97。

签名是清单正文与入口字节的 SHA-256，不是 ADR-0007 里的 Ed25519。空签名只在 `developer_mode` 下放行，并标成 `unsigned_dev`。

## 1 G5 判定项

| # | 判定项 | 实测 | 判定 |
|---|---|---|---|
| (a) | 至少 3 个 JOSM 插件语义移植为 WASM 并可用 | `todo`（选择变化写面板状态）、`utilsplugin`（`add-node`）、`buildings_tools`（`building=yes` 的矩形 `add-way`）。`three_josm_plugins_submit_and_apply` 通过；命令经 `Editor::execute` 写入，建筑可 undo。CLI `plugins` 打出三个插件、面板 `open tasks` 和两条命令 | ✅ |
| (b) | 插件不能越权 | 未授权的命令不入队；`../secret`、绝对路径、反斜杠在读文件前拒绝；HTTP 只接受 HTTPS 且主机在授权表内。探针模块在只授予 `tiles.example` 时：命令数为 0，`http_hits = 1`，日志里没有 `evil`。文件读取先规范化路径，确认仍在插件根目录下，再 `read` | ✅ |
| (c) | 超时或崩溃不影响宿主 | fuel 耗尽的死循环插件被停用；同一轮派发里 todo 仍写入 `open tasks`。`memory.grow` 超过 64 MiB 时增长失败，线性内存保持 65536 字节 | ✅ |
| (d) | 三平台安装包冷启动并打开 1M 数据集 | 仓库里有 Windows zip 脚本、Inno 脚本、Linux tarball + `.desktop`、macOS `.app` 骨架。本轮没有执行这些脚本，没有交叉编译，没有安装包冷启动。同一条导入函数在本机 CLI 上单次打开合成 1M：`nodes=1000000 ways=249750 relations=7813`，`neo-cli.exe` 耗时 **302 ms**（dev，单次，不是 5 次中位数，也不是桌面安装包） | ❌ 未执行 |
| (e) | Presets 与 JOSM 默认预设对照，缺失有记录 | 分组前缀表见 [presets-coverage.md](./presets-coverage.md)。22 个分组计数都大于 0。没有做 `defaultpresets.xml` 逐条对照 | ⚠️ 分组粒度通过，逐条对照未做 |

三个插件是仓内生成的 WAT，导入模块只有 `neo`，不链接 WASI。它们复刻的是上述三个行为，不是 JOSM 插件二进制。

桌面「工具 → 插件、影像与预设」窗口列出这三个名字、一条 TMS URL，以及覆盖表里的缺口。窗口本身不实例化 wasmtime。

## 2 结论

**G5 = Conditional Go**

插件隔离 (a)(b)(c) 在 wasmtime 单测里通过。分组覆盖表已写下，22 个列出的分组都有条目。

不能记为 Go 的部分：(d) 三平台安装包没有构建、没有冷启动；(e) 不是 JOSM 默认预设的逐条清单。MapCSS 增加了 `canvas` / `dashes`，以及 `text` / `icon` 的警告记录，距离 MapCSS 全集仍有缺口。

风险：R7 由 fuel、内存上限和能力求交挡住本轮的探针。R4 保持不拷贝 JOSM XML。R3 只做到分组前缀。R5 本轮只有 Windows。
