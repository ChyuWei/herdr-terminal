[English](README.md) | [简体中文](README.zh.md)

# HerdR Terminal

Herdr Core 的终端客户端。

HerdR Terminal 是 **Herdr Core**（负责运行程序、维持终端存活的 runtime）的终端 UI（TUI）客户端。它和 Herdr App 桌面客户端平级，用来替代旧的 Herdr 1.x TUI。HerdR Terminal 负责屏幕和工作区组织；Herdr Core 负责程序和程序的事实。

**状态：** 已实现，还未发布。客户端连接 Herdr Core，用本地 VT 渲染终端，提供带恢复和 agent 感知的 spaces/tabs/panes 工作区；detach 或重连后程序继续运行。后续功能见下方计划。

## 功能

**已实现**

- **连接**：发现 Core、单条长连接、`hello` 检查、断线重连，运行中的程序不丢。
- **终端**：运行、attach、本地 VT 渲染、键盘和鼠标输入、resize、focus、display facts、基于 credit 的流控，以及 `lost`/`restarted`/`finished` 生命周期。
- **工作区**：创建 spaces、tabs、panes；split/close/move；focus、zoom；重命名 tab 和 space。
- **终端管理**：labels、keep、kill/rm、shell 或命令。
- **Agent**：status、turn、title、foreground、agent 侧栏、peek、跳转、seen/pinned、blocked/finished 提醒。
- **状态**：自己的 `herdr-terminal` 文档、恢复、多屏同步；按 labels 找回终端。
- **恢复**：detach 不结束程序；重连、reattach，`missed` 后重新获取快照。
- **UI**：tab bar、pane 边框、status bar、help 浮层、重命名输入；`Ctrl-b` 前缀键位，`?` 打开帮助。
- **CLI**：`ht` 打开工作区，`ht screen` 运行或重新 attach 单个终端，`ht probe` 输出原始 attach 字节；`--version`、`--help`、`--hc`、`--cols`、`--rows`、`-v`。

**计划中**

- **UI**：命令面板、可配置快捷键、设置、主题；通用 confirm/toast。
- **工作区打磨**：鼠标 resize/drag、tab 重排、scrollback 搜索、复制/粘贴、自定义布局。
- **多机器**：add/connect/list/rename/rm、远程终端和事件。
- **文件**：read/write/edit、diff、外部编辑器、简单文件选择器。
- **发布**：打包、安装、`--skill`、CLI 文档、`ht status` 和 Core/日志诊断。
- **Mods/plugins（postponed）**：后续走 manifest + 外部命令模型，不用 JS/GPUI。
- **Kitty 图片（postponed）**：有需求时再支持。

**不支持**

- 内嵌代码编辑器（用 `$EDITOR`）、Markdown/富文本、没有图形协议的图片、动画、OS 窗口、像素样式、字体选择、GPUI/JS mods、屏幕抓取状态检测、Herdr App 的私有文档/labels、多窗口、telemetry/update UI。

完整的功能范围、已实现功能和计划/不支持功能的原因见[架构文档](docs/02-architecture.md)。

## 设计约束

- **Core 运行程序，客户端绘制屏幕。** Herdr Core 负责 PTY、终端生命周期、原始输出、事件和状态；HerdR Terminal 负责 spaces、tabs、panes、布局和输入。
- **状态只来自 OSC 7501。** 客户端只显示程序通过 Program Status Protocol 报告的状态，不抓屏，也不根据进程名猜测。
- **单条长连接。** 连接断开不代表程序结束：客户端会重连并重新 attach。
- **各自的状态。** 工作区保存在客户端的 `herdr-terminal` 文档中；不会读写 Herdr App 的文档和 `herdr.space`/`herdr.tab` labels。

## 文档

- [Core 与 App 参考](docs/01-core-and-app-reference.md) — Core 和 App 各自的权责、它们如何通信，以及 TUI 可以复用什么；包含 Core、attach、VT、credit、document、label、status 等术语的[术语表](docs/01-core-and-app-reference.md#9-glossary)。
- [架构](docs/02-architecture.md) — HerdR Terminal 的设计、功能范围、已实现功能、不支持的功能和阶段拆分。
- [AGENTS.md](AGENTS.md) — 给 agent 的项目规则、陷阱、当前源码布局和工作方式。

> 设计文档目前为英文，和 Herdr Core / Herdr App 的文档保持一致。

## 命名

- 产品：**HerdR Terminal**
- 仓库、package、Core 文档名：`herdr-terminal`
- 二进制：`ht`
- 许可证：[Apache-2.0](LICENSE)
