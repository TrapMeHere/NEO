# i18n — NEO 界面文本

所有**用户可见文本**都集中在本目录，代码中不再硬编码中文/英文：

| 文件 | 语言 | 说明 |
|---|---|---|
| `zh-CN.json` | 简体中文 | 默认语言；也是 `zh`（不区分地区）的回退表 |
| `zh-TW.json` | 繁體中文（臺灣） | 可缺 key，缺省回退 `zh`（简体表） |
| `en-US.json` | English | 英文；缺 key 同样回退 `zh` |

JSON 为扁平的 `"key": "文本"` 映射，UTF-8 编码。`neo-i18n` crate 用
`include_str!` 在**编译期**嵌入，因此 release exe 不需要携带这些文件。

## 在代码中使用

```rust
use neo_i18n::{t, tf};

ui.label(t("panel.layers"));                       // 直接取文本
let text = tf("status.selected", &[("n", "3")]);   // 占位符 {n}
```

- `t(key)`：取一条文本；当前语言缺失时按 `Lang::fallback` 回退（zh-TW / en-US → `zh`，即简体表），
  再缺失返回 key 本身（便于发现漏翻）。
- `tf(key, &[("name", "value")])`：替换 `{name}` 占位符。
- 语言在运行时切换：界面菜单 **语言 / Language**，或环境变量 `NEO_LANG=zh-CN|zh-TW|en-US|zh|zh-Hant`。
  `zh` 不带地区时按回退规则落到简体中文。
- 新增文本：在 `zh-CN.json` 与 `en-US.json` 加 key；`zh-TW.json` 可稍后补齐（缺失会回退简体）。
  `cargo test -p neo-i18n` 检查 zh-CN/en-US key 一致、zh-TW key 不越界且占位符一致。

## 检查

```powershell
python scripts\check-i18n.py   # 校验代码 key 在 zh-CN/en-US 都存在、zh-TW 不越界，并打印 zh-TW 覆盖率
# cargo test -p neo-i18n：语言代码解析、回退链、zh-TW 覆盖与占位符一致性
```

## 约定

- key 采用 `区域.对象.动作` 命名：`menu.file.save_xml`、`panel.layers`、`app.error.import`。
- 图标由代码单独拼接（如 `format!("{} {}", ph::FOLDER_OPEN, t("menu.file.open_pbf"))`），
  翻译文件只放纯文本。
- 字体覆盖示例、OSM 数据名（如 `Monaco`）等非界面文案不放入翻译表。
