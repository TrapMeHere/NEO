# i18n — NEO 界面文本

所有**用户可见文本**都集中在本目录，代码中不再硬编码中文/英文：

| 文件 | 语言 | 说明 |
|---|---|---|
| `zh-CN.json` | 简体中文 | 默认语言 |
| `en-US.json` | English | 英文 |

JSON 为扁平的 `"key": "文本"` 映射，UTF-8 编码。`neo-i18n` crate 用
`include_str!` 在**编译期**嵌入，因此 release exe 不需要携带这些文件。

## 在代码中使用

```rust
use neo_i18n::{t, tf};

ui.label(t("panel.layers"));                       // 直接取文本
let text = tf("status.selected", &[("n", "3")]);   // 占位符 {n}
```

- `t(key)`：取一条文本；当前语言缺失时回退到中文，再缺失返回 key 本身（便于发现漏翻）。
- `tf(key, &[("name", "value")])`：替换 `{name}` 占位符。
- 语言在运行时切换：界面菜单 **语言 / Language**，或环境变量 `NEO_LANG=zh-CN|en-US`。
- 新增文本：同时在 `zh-CN.json` 与 `en-US.json` 加 key（`neo-i18n` 的测试会检查两边 key 一致）。

## 检查

```powershell
python scripts\check-i18n.py   # 校验代码里 t()/tf() 用到的 key 都存在，且中英 key 一致
# cargo test -p neo-i18n 在 test 分支：目录加载、回退、占位符、双语 key 对齐
```

## 约定

- key 采用 `区域.对象.动作` 命名：`menu.file.save_xml`、`panel.layers`、`app.error.import`。
- 图标由代码单独拼接（如 `format!("{} {}", ph::FOLDER_OPEN, t("menu.file.open_pbf"))`），
  翻译文件只放纯文本。
- 字体覆盖示例、OSM 数据名（如 `Monaco`）等非界面文案不放入翻译表。
