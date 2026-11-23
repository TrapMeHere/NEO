# OSM 同步（下载 / 上传）

## 前置：登录

在 **帮助 → 设置… → OSM 账户** 完成 OAuth 登录（见 [设置与多语言](settings-and-i18n.md)）。

## 下载当前区域

1. **文件 → 同步（下载 / 上传）…**
2. 窗口显示当前视口 bbox 与账户
3. 点「下载当前区域」

NEO 调用 `GET /api/0.6/map?bbox=…`，用服务端数据**替换**当前图层（未保存的本地修改请先导出/保存）。
下载结果标记为 `OSM API download`。

## 上传本地修改

1. 填写 changeset 注释
2. 点「上传本地修改」

流程：`create changeset` → `POST /changeset/{id}/upload`（osmChange）→ `close changeset`。
成功后面板显示服务端确认的对象数。

## 409 冲突

上传返回 409 时，同步窗口列出冲突对象（类型/id/版本），提供两种策略：

- **保留本地并重试（KeepLocal）**：把本地对象版本对齐到服务端版本后重新上传（覆盖远端修改）
- **重新下载（放弃本地修改，TakeRemote）**：用服务端数据替换图层，再重新编辑/上传

手合并（按字段/几何选择）仍待后续；`neo-editor::MergeCase` 与三方标签合并已就绪。
