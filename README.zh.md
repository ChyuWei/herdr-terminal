[English](README.md) | [简体中文](README.zh.md)

# HerdR Terminal

Herdr Core 的终端客户端。

HerdR Terminal 是 **Herdr Core**（负责运行程序、维持终端存活的 runtime）的终端 UI（TUI）客户端。它和 Herdr App 桌面客户端平级，用来替代旧的 Herdr 1.x TUI。HerdR Terminal 负责屏幕和工作区组织；Herdr Core 负责程序和程序的事实。

**状态：** 已实现，还未发布。客户端连接 Herdr Core，用本地 VT 渲染终端，提供带恢复和 agent 感知的 spaces/tabs/panes 工作区，并采用 Space → Pane 界面：pane 从顶部开始，左侧栏列出 spaces 和 agents，底部只有一行显示模式和当前终端。日常使用能力包括鼠标、有上限的 scrollback 和搜索、选择与复制/粘贴、命令面板、窄屏 pane 切换条、侧栏抽屉和键盘 resize，以及带深/浅色外观（另提供 Catppuccin 别名和命名色 `terminal` 外观）的启动配置；此外还有 Core 范围内的终端管理器、面向 Core 机器的机器管理器（远程终端位于同一工作区中），以及由 Core 读取和写入的文件查看器、外部编辑器、行级 diff 和写回。文件选择器及其余后续功能见下方计划。

**兼容性：** 支持 Herdr Core 0.1.17 及更新版本，协议 generation 1。连接时若 Core 更旧或版本无法解析，客户端会给出更新提示并拒绝连接；缺少必需方法或 feature 的 Core 也会被拒绝。`resize_notifications` 是连接必需的能力，并非可选（observer attach 没有回退路径）；其余能力根据 Core 在 `hello` 中声明的 features 启用，而不是检查版本字符串。

## 功能

**已实现**

- **连接**：发现 Core、单条长连接、`hello` 检查、断线重连，运行中的程序不丢。
- **终端**：运行、attach、本地 VT 渲染、键盘和鼠标输入、resize、focus、display facts、基于 credit 的流控，以及 `lost`/`restarted`/`finished` 生命周期。新的本地终端会从当前 pane 由 Core 报告的 cwd 启动，否则使用客户端进程的 cwd（若为 `/` 或不可读则用 `$HOME`）；在机器上启动时不发送本地 cwd，使用该机器的 home（替换终端沿用退出终端的 cwd），恢复的工作区中显式指定的 `ht PROG` 也跟随当前 pane。本屏启动的终端在程序退出时会自动关闭 pane（断开 stream，并尽可能用 `terminal.rm` 移除已结束记录）；若它是 tab 的最后一个 pane，会在同一 space/tab 中启动一个新的 shell，`machine/t7` pane 仍在同一机器上，后台 tab 不会因此切换为 active；几乎立即退出的 shell 不会再次替换，而已在恢复或采用时即为 finished 的记录会保留最终画面直到关闭。
- **文本渲染**：默认开启 grapheme clustering，与 Core 和 Herdr App 一致，因此 `✍️`、`❤️` 等带 variation selector 的 emoji 是一个宽两格的 cluster，后面紧跟一个零宽占位格，不会拆成不同单元格。
- **初始终端尺寸**：启动新终端时会带上 attach 时将使用的网格（初始 shell、split/tab/space、显式程序、`ht screen`、重连或退出后的替换终端和 `ht probe`），因此只读取一次尺寸的程序也能以正确的布局启动。所有受支持的 Core 都会应用该字段，因此不需要版本判断。
- **工作区**：创建 spaces、tabs、panes；split/close/move；focus、zoom；重命名 tab 和 space。没有 tab bar：tabs 仍保留在工作区模型和文档中，通过 `prefix n`/`p`、`prefix 1..9` 和按需打开的 `prefix T` 选择器切换（显示序号、名称、pane 数和当前标记；Enter 切换，Esc/q 关闭）；space 行上的 tab 数量只是信息展示。
- **终端操作**：labels、keep、kill/rm、shell 或命令。
- **终端管理器**：`prefix t` 打开 Core 范围内所有 live 终端的列表，包含已显示标记、OSC 7501 状态（未报告时为 `unknown`）、存活时间、地址，以及可选的名称/标题/父终端/机器/cwd；按 blocked/error、working、done、idle、unknown 排序，并支持大小写不敏感的子串过滤。`Enter` 通过采用现有地址显示未显示的终端（不会启动新程序），或隐藏已显示的终端（detach 并移除 pane，绝不 kill/rm）；`p` 通过 observer attach 只读预览，和 agent peek 不同。选择和过滤属于本地 UI 状态。
- **机器管理器**：`prefix m` 打开 Core 已知机器的模态列表，数据来自 `machine.list`，包含名称、状态（connecting、connected、needs_login、needs_update、unreachable 或 unknown）、处于当前状态的时长和 Core 给出的原因（如有）、target、host/version、session，以及 temporary/via 标记；按 connected、connecting、needs_login、needs_update、unreachable、unknown 排序，同状态中永久机器在前，并支持大小写不敏感的子串过滤。`Enter` 连接或重试，`a` 按 target 添加（原样交给 Core），`r` 重命名，`d` 经确认后移除，`n` 在所选机器上启动终端。远程终端使用 `machine/t7` 地址，像本地 pane 一样位于当前工作区，并从本地文档恢复；机器连接（SSH 或 via 命令）、拨号、退避和中继流由 Core 负责，客户端按 pane label 跟随机器重命名并保留 pane 和 stream。列表只在浮层打开时、每次操作后和定时器中刷新；Core 没有机器状态事件。
- **文件**：`prefix o` 打开模态文件浮层。文件通过 Core 读写，使用当前 pane 的机器（存在时作为 `on`）和 cwd；路径输入从该 cwd 开始，不提供目录列表或选择器（以 `/` 或 `\` 结尾的输入会被直接拒绝）。查看器只读，用 `file.read` 每次读取 400 行，显示机器、路径、已加载范围和总行数；j/k 或方向键、PgUp/PgDn、g/G 或 Home/End 滚动并在跨越页时读取下一页/上一页；Esc 关闭。`e` 通过 Core 读取整个文件，写入唯一的本地临时副本并用 `$VISUAL` 或 `$EDITOR` 打开（按空白拆分，不回退到 `vi`），期间挂起并恢复 TUI；返回后显示 `+`/`-` 行级 diff。`w`/Enter 写入，`q`/Esc 丢弃。写入前会重新读取并与首次读取比较；文件已变化时显示冲突，`o` 是显式覆盖。这个比较不是原子操作：Core 没有文件版本，重新读取与写入之间发生的变化不会被发现。没有冲突时，单个连续替换使用 `file.edit`，否则使用 `file.write`（包括显式覆盖）。Core 因内联读取上限而截断的整文件读取不能编辑或写回，但查看器仍可分页浏览。文件内容不进入文档或 labels；只有编辑器的临时副本会写到本地。`file.read`、`file.write`、`file.edit` 是连接必需的方法。
- **Agent**：status、turn、title、foreground、spaces/agents 侧栏、本地 VT peek、跳转、seen/pinned、blocked/finished 提醒。侧栏在至少 90 列时占据左侧，宽度为视口四分之一并限制在 20–30 列；`SPACES` 行显示序号、名称和 tab 数（space 有 folder 时再加一行 folder），随后跨 space 的 `AGENTS` 索引为每个 agent 显示两行：标题和 Core 状态词，以及 `space · address`。agents 按 pinned、blocked、done、working、idle 排序；普通 shell 不会列出，所有状态词只来自 OSC 7501。`prefix a` 聚焦侧栏，j/k 或方向键和 g/G 移动，Enter 切换 space 或跳转到 agent，`i` peek，`p` 固定，`u` 标为未读，`a`/Esc 返回焦点；行的两行都可点击。低于 90 列时侧栏隐藏，`prefix a` 以抽屉形式覆盖在 panes 上；点击外部会关闭它，打开期间鼠标输入全部被它消费，不会到达下面的 pane。它不会调整 pane 大小、改变 split 树或写入文档。聚焦侧栏在 seen/attention 上视为遮挡 panes，但在宽屏上不会让鼠标变成模态。
- **状态**：自己的 `herdr-terminal` 文档、恢复、多屏同步；按 labels 找回终端。
- **恢复**：detach 不结束程序；重连、reattach，`missed` 后重新获取快照。
- **鼠标**：点击可聚焦 pane、侧栏行或窄屏 pane 切换条上的 chip；底部行上的点击会被消费；在宽屏且未 zoom 的 tab 上拖动 split 边界可调整大小；chrome 不会发送给程序。程序不跟踪鼠标时滚轮滚动本地历史；Shift 拖动在程序跟踪鼠标时也做本地选择。
- **Scrollback 和搜索**：每个 pane 保存有上限的历史，attach 时向 Core 请求历史；滚轮和 Shift-PgUp/PgDn/Home/End 滚动，普通按键回到 live 底部。`prefix f` 做字面、大小写不敏感的子串搜索（不支持正则），显示匹配数并支持下一个/上一个。历史按行数封顶并有字节预算；VT 按页裁剪，保留的行数可能比上限多约一页。
- **选择和剪贴板**：`prefix [` 或拖动选择；copy 模式下 h/j/k/l 移动一格，`w`/`b`/`e` 按词移动，`v`/space 在末端重新锚定，`y`/Enter 复制，`/` 或 `?` 打开 scrollback 搜索并结束选择，Esc 取消；默认 OSC 52，可配置原生 helper（`pbcopy`、`wl-copy` 或 `xclip`）和 bracketed paste。选择以单元格为单位，不提供按词或按行的选择模式。
- **模式**：底部唯一一行显示一个 chip——`NORMAL`、`PREFIX`、`COPY`、`RESIZE`，或当活动 tab 已 zoom 且没有其他模式时的 `ZOOM`——后面是当前终端的身份信息和上下文提示。`prefix r` 仅在宽屏、未 zoom 且 tab 有多个 pane 时进入 resize 模式；h/l 或 j/k 将当前 split 移动一格，Shift 移动五格，Esc/q 退出，其他按键退出且不执行操作。窄屏或 zoom 状态会提示 resize 不可用。打开浮层或搜索、聚焦侧栏或重连会清除模式，因此 chip 和按键始终一致。zoom 时只绘制当前 pane。
- **命令面板**：`prefix :` 对现有命令表做大小写不敏感过滤，显示当前键位提示；Up/Down 移动选择，Enter 执行；`j`/`k` 输入到过滤器。
- **UI**：默认界面是 Space → Pane：没有窗口栏、品牌标题、tab bar 或每个 pane 的页脚。panes 从第 0 行开始，每个 pane 在上边框保留一行身份信息（焦点标记、两位数布局编号、名称/标题，右侧是 OSC 7501 状态词和地址），聚焦 pane 使用 mint 色边框和表头。底部唯一一行横跨整个视口，显示模式 chip、当前终端身份（Core 报告时为 `address · app · cwd`，否则为地址和名称）以及上下文提示；低于 90 列时，panes 上方多一行窄屏 pane 切换条。界面默认使用深色外观（`default`/`dark`）或浅色外观（`light`），`catppuccin`/`catppuccin-latte` 保留早先的 Catppuccin 配色，`terminal` 使用命名色外观；颜色会降级到 256/16 色终端。`Ctrl-b` 前缀键位和 `?` 帮助仍然可用；终端单元格保留程序颜色。
- **设置**：启动时读取 `$XDG_CONFIG_HOME/herdr-terminal/config.json`（HOME 回退），支持 theme、scrollback、mouse、clipboard 和前缀键位；`default` 和 `dark` 选择深色外观，`light` 选择浅色外观，`catppuccin` 和 `catppuccin-latte` 保留早先的 Catppuccin 配色，`terminal` 使用命名色外观；坏值会警告并回退。设置只在启动时读取，没有设置 UI 或热重载。
- **性能**：每帧的处理量、scrollback 和搜索都有上限，大输出时输入仍保持响应。
- **CLI**：`ht` 打开工作区，`ht screen` 运行或重新 attach 单个终端，`ht probe` 输出原始 attach 字节；`--version`、`--help`、`--hc`、`--cols`、`--rows`、`-v`。
- **发布**：macOS Apple Silicon 发布包，包含 `ht`、许可证和生成的第三方 notices；GitHub Actions CI 和由 tag 触发的 GitHub release 工作流。发布包要求 Herdr Core 0.1.17 或更新版本，且不包含 Core。

**计划中**

- **UI**：通用 confirm/toast。
- **工作区打磨**：tab 重排、自定义布局、无障碍（accessibility）。
- **文件**：目录列表和简单文件选择器。
- **发布**：安装脚本、`--skill`、CLI 文档、`ht status` 和 Core/日志诊断。
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
- **本地 UI 状态。** scrollback、搜索、选择、模式状态、侧栏选择和主题只留在客户端，不写入共享文档或每屏视图；设置从本地文件在启动时读取一次，没有热重载。

## 文档

- [Core 与 App 参考](docs/01-core-and-app-reference.md) — Core 和 App 各自的权责、它们如何通信，以及 TUI 可以复用什么；包含 Core、attach、VT、credit、document、label、status 等术语的[术语表](docs/01-core-and-app-reference.md#9-glossary)。
- [架构](docs/02-architecture.md) — HerdR Terminal 的设计、功能范围、已实现功能、不支持的功能和计划工作。
- [AGENTS.md](AGENTS.md) — 给 agent 的项目规则、陷阱、当前源码布局和工作方式。

> 设计文档目前为英文，和 Herdr Core / Herdr App 的文档保持一致。

## 命名

- 产品：**HerdR Terminal**
- 仓库、package、Core 文档名：`herdr-terminal`
- 二进制：`ht`
- 许可证：[Apache-2.0](LICENSE)
