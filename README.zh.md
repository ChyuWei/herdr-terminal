[English](README.md) | [简体中文](README.zh.md)

# HerdR Terminal

Herdr Core 的终端客户端。

HerdR Terminal 是 Herdr Core 的 TUI 客户端，和 Herdr App 平级，用来替代旧的 Herdr 1.x TUI。它负责终端屏幕和工作区组织；Herdr Core 负责程序和程序的事实。

**状态：** 设计阶段；Cargo workspace 已初始化，TUI 还没有实现。

## 功能

**v1 — 首个可用版本**

- **连接**：发现 Core、单条长连接、`hello` 检查、断线重连，运行中的程序不丢。
- **终端**：运行、attach、本地 VT 渲染、键盘和鼠标输入、resize、focus、display facts。
- **工作区**：spaces、tabs、panes、split/close/move、focus、zoom。
- **终端管理**：新建/关闭/重命名、labels、keep、kill/rm、shell 或命令。
- **Agent**：status、turn、title、foreground、agent 列表/侧栏、peek、seen/pinned。
- **UI**：tab bar、pane 边框、status bar、命令面板、help、toast/confirm/prompt、主题。
- **状态**：自己的 `herdr-terminal` 文档、恢复、多屏同步；按 labels 找回终端。
- **恢复**：`lost`、`restarted`、`finished`、`missed`、heartbeat、reattach。
- **诊断**：版本、Core 状态、日志、`ht status`。

**后续**

- **工作区打磨**：鼠标 resize/drag、scrollback 搜索、复制/粘贴、自定义布局。
- **多机器**：add/connect/list/rename/rm、远程终端和事件。
- **文件**：read/write/edit、diff、外部编辑器、简单文件选择器。
- **发布**：打包、安装、`--skill`、CLI 文档。
- **Mods/plugins（postponed）**：后续走 manifest + 外部命令模型，不用 JS/GPUI。
- **Kitty 图片（postponed）**：有需求时再支持。

**不支持**

- 内嵌代码编辑器（用 `$EDITOR`）、Markdown/富文本、没有图形协议的图片、动画、OS 窗口、像素样式、字体选择、GPUI/JS mods、屏幕抓取状态检测、Herdr App 的私有文档/labels、多窗口、telemetry/update UI。

完整状态表和原因见[架构文档](docs/02-architecture.md)。

## 文档

- [Core 与 App 参考](docs/01-core-and-app-reference.md) — Core 和 App 各自的权责、它们如何通信，以及 TUI 可以复用什么。
- [架构](docs/02-architecture.md) — HerdR Terminal 的设计、功能范围、不支持的功能和阶段拆分。
- [AGENTS.md](AGENTS.md) — 给 agent 的项目规则、陷阱和工作方式。

> 设计文档目前为英文，和 Herdr Core / Herdr App 的文档保持一致。

## 命名

- 产品：**HerdR Terminal**
- 仓库、package、Core 文档名：`herdr-terminal`
- 二进制：`ht`
- 许可证：[Apache-2.0](LICENSE)
