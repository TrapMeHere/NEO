# 安全策略（Security Policy）

## 支持版本

| 版本 | 支持 |
|---|---|
| 最新 release（0.1.x） | ✅ |
| 更早版本 | ❌（请升级） |

## 报告漏洞

请**不要**在公开 Issue 中披露安全问题。优先使用 GitHub 的私密漏洞报告：

1. 打开仓库 **Security → Advisories → Report a vulnerability**
   （`https://github.com/novolife/neo/security/advisories/new`）；
2. 或发邮件给维护者（在 GitHub 个人资料中可查），主题前缀 `[NEO Security]`。

报告请包含：影响版本、复现步骤、影响面（数据丢失/任意代码执行/崩溃等）、
PoC（如可提供）与你的联系方式。

## 响应流程

- 48 小时内确认收到；
- 7 天内给出初步评估与修复计划；
- 修复后发布 patch release，并在 GitHub Security Advisory 中公开致谢（如你同意）。

## 范围

- `neo-io` 的 PBF/XML/osmChange 解析（恶意文件导致崩溃/OOM/越界）
- `neo-editor` 的 journal 恢复、会话文件（.jos/.joz）解析
- `neo-render`/wgpu 集成、字体/图标资源加载
- 依赖供应链（`cargo deny`/Dependabot）

## 不在范围

- 上游 OSM API/数据服务的问题
- NVIDIA Overlay/驱动弹窗（见 FAQ）
- 用户本机的第三方软件冲突
