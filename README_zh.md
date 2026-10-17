<h1 align="center"><img src="logo.svg"/></h1>

<p align="center">
	<a href="README.md">English</a>
	&nbsp;&nbsp;&nbsp;|&nbsp;&nbsp;&nbsp;
	<a href="README_zh.md">简体中文</a>
</p>

<p align="center" style="color:gray;">
	基于 Rust 的网易云音乐 TUI 客户端，内置全屏播放页。
</p>

<p align="center">
    <img src="https://img.shields.io/badge/Language-Rust-orange?logo=rust&logoColor=white" alt="Rust">
    <img src="https://img.shields.io/badge/Platform-Linux%20%7C%20Windows%20%7C%20macOS-informational?logo=linux&logoColor=white" alt="Platform">
    <img src="https://img.shields.io/badge/License-AGPL--3.0-blue?logo=opensourceinitiative&logoColor=white" alt="License">
    <img src="https://img.shields.io/github/stars/professor-lee/CNMPlayer?style=flat&label=Stars&color=FFC700&logo=github&logoColor=white" alt="Stars">
    <img src="https://img.shields.io/github/forks/professor-lee/CNMPlayer?style=flat&label=Forks&color=60adff&logo=git-fork&logoColor=white" alt="Forks">
    <img src="https://img.shields.io/github/v/release/professor-lee/CNMPlayer?color=32cd32&label=Release&logo=github-actions&logoColor=white" alt="Release">
    <img src="https://img.shields.io/github/last-commit/professor-lee/CNMPlayer?color=rebeccapurple&logo=git&logoColor=white" alt="Last Commit">
	<img src="https://img.shields.io/github/commit-activity/m/professor-lee/CNMPlayer?style=flat&color=FF69B4&logo=github" alt="Commit Activity">
	<img src="https://img.shields.io/github/languages/code-size/professor-lee/CNMPlayer?style=flat&color=blueviolet" alt="Code Size">
</p>

## 项目概述

CNMPlayer（Customized Netease Music Player）是一个运行在终端中的网易云音乐客户端。
同一个进程里有两套界面：

- **主程序界面**：登录、首页推荐、歌单 / 作者 / 搜索页、可滑出的侧边栏，以及底部 5 行的折叠播放栏；
- **全屏播放页**：封面、歌词、歌单浮层、10 段均衡器与可视化。全屏快捷键（默认 `Ctrl+F`）进入该页面，再按 `Ctrl+F` 或 `Esc` 返回主程序。

播放本身由主程序负责：带本地缓存的流式播放、播放记忆、私人漫游、按 VIP 权限裁剪的音质，以及由其它界面绘制的可视化。

> CNMPlayer 是非官方客户端；音乐内容版权归原权利人所有，播放缓存与下载仅供个人离线使用。使用前请先读 [DISCLAIMER_zh.md](DISCLAIMER_zh.md)。

## 应用截图

<p align="center" width="100%">
	<img src="docs/screenshots/host-home.png" width="45%" alt="主程序首页">
	&nbsp;
	<img src="docs/screenshots/host-sidebar.png" width="45%" alt="首页侧边栏">
	<br><sub>主程序首页；侧边栏（用户创建 / 用户收藏歌单）</sub>
</p>

<p align="center" width="100%">
	<img src="docs/screenshots/fullscreen-bars.png" width="45%" alt="全屏播放页：bars 频谱">
	&nbsp;
	<img src="docs/screenshots/fullscreen-oscilloscope.png" width="45%" alt="全屏播放页：示波器波形">
	<br><sub>全屏播放页——<code>bars</code> 频谱；<code>oscilloscope</code> 示波器波形</sub>
</p>

<p align="center" width="100%">
	<img src="docs/screenshots/fullscreen-vector.png" width="45%" alt="全屏播放页：矢量模式">
	&nbsp;
	<img src="docs/screenshots/host-search.png" width="45%" alt="主程序搜索页">
	<br><sub>全屏 <code>vector</code> 矢量模式；带后缀的搜索结果</sub>
</p>

## 主要功能

### 账号

- 二维码（`F1`）、账号（用户名 / 邮箱 + 密码，`F2`）和手机号 + 验证码（`F3`）登录
- 会话保存在 `auth/session.toml`，下次启动时校验
- 设置弹窗里有「退出登录」（清除登录 cookie、播放记忆与私人漫游数据）

### 浏览

- 首页：推荐磁贴网格，`每日推荐`、`私人雷达`、`私人漫游` 三块始终占据最前面的位置；`home_more_recommend` 会展开其余推荐
- 首页侧边栏（开关快捷键默认 `P`）：用户创建与用户收藏的歌单，每次获取 100 条，滚动到底部自动续页；`Ctrl+Up/Down` 切换分区，`Enter` 打开，`Esc` 收起
- 歌单页——专辑也复用它，没有独立的专辑页；头部（封面、标题、作者、简介、曲目数）加虚拟化曲目列表，歌曲每次获取 100 首。歌单与播放列表共用绑定来源的游标，任一侧续页都会同步追加到另一侧
- 作者页：头像、名称、热门歌曲 / 专辑 / EP / 单曲数量，以及各分类的磁贴网格
- 搜索页：关键词同时搜作者、歌单与单曲，末尾追加后缀可限定类型（见 [搜索语法](#搜索语法)）
- 私人漫游：每日刷新时把上次播放的歌曲保留在首位；播放到列表末尾会续拉新歌（连拉三次并去重）追加到队尾；磁贴封面跟随当前播放的漫游歌曲
- 键盘导航：`Enter` 打开或播放，`Esc` / `Left` 返回，`Tab` / `Down` 与 `Shift+Tab` / `Up` 移动，`PageUp` / `PageDown` 翻页
- 鼠标：滚轮滚动，单击聚焦，双击打开；折叠播放栏的上一首 / 播放暂停 / 下一首、收藏与循环模式按钮，以及进度条可点击

### 搜索语法

搜索框（`Ctrl+S`）默认同时搜作者、歌单与单曲，末尾追加后缀可限定单一类型。
`@author` 与 `@artist` 同义，且 `@author` 后不带关键词会列出已关注作者。

| 查询 | 结果 | `Enter` |
| --- | --- | --- |
| `关键词` | 最相关的作者、歌单与单曲 | 看当前行：播放歌曲 / 打开作者页 / 打开歌单 |
| `关键词@single` | 单曲 | 从该行开始播放，并把整个结果集设为队列 |
| `关键词@album` | 专辑 | 以歌单页样式打开该专辑 |
| `关键词@list` | 歌单 | 打开该歌单 |
| `关键词@author` / `关键词@artist` | 歌手 | 打开作者页 |
| `@author`（不带关键词） | 已关注作者 | 打开作者页 |

作者与歌单只取最相关的 5 条且不参与分页；只有单曲分区会在滚动到底时继续追加。

### 播放

- 流式播放：边下边播，完整响应写入成功后才成为缓存文件，因此半成品或被取消的传输不会被当作缓存播放；进度条的已缓冲段就是下载进度
- 可跳转进度（进度条点击或全屏页内操作），跳转期间进度条有脉冲动画
- 播放记忆（`playback_memory`）：队列、当前索引、循环模式与队列来源会在每次切歌时保存，并在启动时恢复；恢复的歌曲从头开始播放
- 按 VIP 权限裁剪的音质（`audio_quality`）：从 `standard` 到 `jymaster` 共 9 档；非 VIP 账号会被限制到 `exhigh`
- 10 段均衡器，±12 dB（`eq_bands_db`），在全屏 EQ 弹窗里调整，实时作用于播放
- 收藏 / 取消收藏：全屏页与折叠播放栏都可操作
- 循环模式：顺序 → 随机 → 列表循环 → 单曲循环
- 歌单播放进入已加载列表最后三首时预取下一页；到达尚未完成的分页边界时会等待续页，不会提前结束或回卷
- Linux 媒体控制（MPRIS，播放器名 `cnmplayer`），包含元数据与封面

### 下载

- 下载到系统音乐目录下的 `cnmplayer/`（`download_path` 可改；系统没有音乐目录时回退 `~/Music/cnmplayer/`）
- 入口：主程序单曲行上的下载按钮、全屏页标题行爱心左侧的按钮、`Ctrl+Alt+D`（主程序）与 `Ctrl+D`（全屏页）；再按一次会取消下载并删掉半成品文件
- 同一时间只跑一个下载任务，多次触发会排队依次执行
- 文件名 `标题 - 作者 - 专辑.<mp3|flac>`，并写入标签：标题、作者、专辑、曲目号、日期、内嵌封面与歌词（无损写 Vorbis Comment，mp3 写 ID3v2）
- 设置弹窗的「下载设置」可调下载音质与下载路径：绝对路径生效；留空或填字面量 `Null` 即禁用下载（界面不再显示下载入口，「音质」灰置）

### 可视化

- `hidden`（设置弹窗里显示「关闭」）——全屏页右侧整块收起：可视化与歌词都不画，歌曲信息区撑满整个终端宽度
- `lyrics`（显示「仅歌词」）——只显示歌词，不画可视化
- `bars`——内部忠实移植 Cava 算法的 Rust 频谱条，使用八级子格高度绘制；默认档位，无需外部 `cava` 可执行文件
- `oscilloscope`——真 PCM 示波器；暂停、停止或音频中断时平滑收回平线，恢复后重新张开
- `vector`——李萨如矢量示波器：横向 x 轴 = 左声道 L，纵向 y 轴 = 右声道 R（向上恒为正），缩放基准取本曲开播以来的最响段落，恰好撑满面板。暂停或突断静音时图形炸开成飞散的盲文点，停稳后轻微明灭，直到恢复播放
- 折叠播放栏绘制 10 格盲文迷你频谱（`lyrics` 与 `hidden` 两档中保持空白）；窄窗则用 400 ms Momentary LUFS 计量驱动双声道音量条（显示范围 −60…0 LUFS）

`bars` 与迷你频谱分析的是 CNMPlayer 自己解码的播放 PCM，抽头位于十段 EQ 之后、播放音量之前——不采集麦克风、系统输出或其它应用的声音。频谱按 [Cava](https://github.com/karlstav/cava) 的 [`6d43df3b2c7882122585c02c064b20009842a6f8`](https://github.com/karlstav/cava/tree/6d43df3b2c7882122585c02c064b20009842a6f8) 提交忠实移植为 Rust，FFT 后端为纯 Rust 实现（`realfft`）。归属与完整许可条款见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

### 小窗口模式

默认开启（`small_window_display = false` 可关闭）。它作用于主程序的内容页，终端降到阈值以下立即接管：

| 终端尺寸 | 行为 |
| --- | --- |
| 宽度 ≥ 32 且高度 ≥ 12 | 正常界面 |
| 宽度 ≥ 32，5 ≤ 高度 < 12 | 扁窗：上方显示页面歌词，下方为 5 行折叠播放栏 |
| 宽度 ≥ 32，高度 = 5 | 扁窗、一次只显示一个面板：开关快捷键（默认 `Alt+X`）在播放栏与歌词之间横向滑动切换 |
| 宽度 < 32，高度 ≥ 12 | 窄窗：铺满宽度的双声道 LUFS 音量条 |
| 两个方向都过小 | 显示「终端窗口过小」 |

进入小窗口会关闭侧边栏与已打开的弹窗；在小窗口内设置、搜索框、侧边栏都无法打开，只有退出、上一首、下一首、播放暂停、循环模式、折叠栏收藏、小窗口开关快捷键，以及扁窗播放栏的鼠标目标（上一首、播放暂停、下一首、收藏、循环模式、点击进度跳转）仍然可用。

### 界面

- 主题：从 `themes/*.toml` 动态加载——内置 20 款，默认 `frappe`；自己丢一个 toml 进去即可加入循环。校验不过的文件会被跳过，选中的主题损坏时回退默认主题
- 界面语言：`zh` / `en`
- 22 个可重绑快捷键，带冲突检测；`Ctrl+Alt+R` 恢复默认
- 透明背景、封面边框、提示行开关
- 启动：先出加载页（ASCII 标题 + 进度条），登录恢复、推荐加载等网络步骤在后台按步推进；登录态不可用时进入登录页
- about 弹窗含盲文形象画，里面还藏了一个彩蛋（`easter-egg` cargo feature，默认编入，可用 `--no-default-features` 剔除）

## 安装

### Arch Linux（AUR）

| 包名 | 内容 |
| --- | --- |
| `cnmplayer-bin` | 最新 GitHub Release 的预编译二进制 |
| `cnmplayer` | 用最新 release tag 从源码构建 |
| `cnmplayer-git` | 跟踪 `develop` 分支从源码构建 |

```bash
# 以 paru 为例
paru -S cnmplayer-bin
```

### 预编译包

每个版本都会在 [Releases](https://github.com/professor-lee/CNMPlayer/releases) 发布 `CNMPlayer_vX.Y.Z_linux_amd64.tar.xz`、`CNMPlayer_vX.Y.Z_linux_aarch64.tar.xz` 与 `SHA256SUMS`。两种压缩包都是平铺结构，内含 `cnmplayer` 可执行文件、`LICENSE` 与 `THIRD_PARTY_NOTICES.md`（包括 Cava 和 Rust FFT 依赖的完整 MIT 声明）。

```bash
# 把 SHA256SUMS 与下载的压缩包放在同一目录，校验已下载的架构。
sha256sum --check --ignore-missing SHA256SUMS
tar -xJf CNMPlayer_vX.Y.Z_linux_amd64.tar.xz
./cnmplayer
```

### 从源码构建

```bash
git clone https://github.com/professor-lee/CNMPlayer.git
cd CNMPlayer
cargo build --release
./target/release/cnmplayer
```

Debian/Ubuntu 上的构建依赖（与 CI 使用的列表一致）：

```bash
sudo apt update
sudo apt install -y build-essential cmake pkg-config \
  libasound2-dev libchafa-dev libpipewire-0.3-dev libssl-dev libglib2.0-dev libclang-dev
```

`libchafa-dev` 要求 chafa ≥ 1.8.0（图像渲染通过 `pkg-config` 探测），`libclang-dev` 与 `libpipewire-0.3-dev` 则是 PipeWire 音频后端在构建期生成绑定所需。`libasound2-dev` 只在构建期需要：`cpal` 在 Linux 上无条件编译其 ALSA 后端，而播放本身走 PipeWire。

### 运行要求

- Linux 上的 PipeWire 音频（ALSA 后端已弃用），以及运行时的 chafa 共享库
- 播放与导航图标要求 Nerd Font；程序不根据 `TERM` 猜测字形是否可用

## 首次运行与资源目录

首次运行时，程序会在系统配置目录下创建资产目录；Linux 上通常是 `~/.config/cnmplayer`。
如果设置了 `CNMPLAYER_ASSET_DIR`，则会改用该目录作为资产根目录。

该目录下会有（随使用逐步生成）：

- `config/default.toml`：程序、播放、快捷键与缓存配置
- `themes/*.toml`：主题文件（key 取文件内 `name` 字段，缺省时取文件名）；额外丢进来的 `.toml` 会自动变为可选
- `auth/session.toml`：持久化登录 cookie
- `playback/session.toml`：播放记忆的队列（`playback_memory` 开启时写入）
- `private_roam/session.toml`：私人漫游列表、最后播放位置与缓存封面

缓存目录默认使用系统缓存目录（Linux 上是 `~/.cache/cnmplayer`），可用 `cache.path` 指定。其中包含：

- `audio/<song_id>__<quality>.audio`：已完成的下载；下载中的文件是 `.part`，失败或取消时会被删除（进程被强杀时可能残留）
- `cover/`：当前播放歌曲的封面图缓存
- `mpris_art/`：为 MPRIS 播放器导出的封面文件
- `Player.log` 与 `Player.stderr.log`：程序日志与原生音频后端的 stderr（都限制在 4 MB）

## 配置

仓库中的 `config/default.toml` 在构建时内嵌，是默认值的唯一来源：首次生成配置、`Config::default()` 和缺失字段补全（含嵌套缓存配置）。用户配置覆盖模板值，缺失字段会在启动时补全并写回。解析失败、枚举值无效和 `ui_fps = 0` 会报错，不替换用户文件；`page_lyrics_pos_x` / `page_lyrics_pos_y` 越界时会被钳位。

| 配置项 | 默认值 | 取值 / 说明 |
| --- | --- | --- |
| `theme` | `frappe` | `themes/*.toml` 里的任意主题 key（内置 20 款，自动识别自定义文件）；选中的主题文件损坏时回退默认主题 |
| `language` | `zh` | `zh`、`en` |
| `visualize` | `bars` | `hidden`（设置里显示「关闭」）、`lyrics`（「仅歌词」）、`bars`、`oscilloscope`、`vector`；所有可视化均为内部实现 |
| `transparent_background` | `true` | 使用终端背景 |
| `album_border` | `true` | 全屏封面边框 |
| `graphics_protocol` | `halfblocks` | `off`（ASCII 字符画）或 `halfblocks`（彩色半块渲染封面；旧值 `auto` / `sixel` / `kitty` / `iterm2` 按此项读取） |
| `show_hints` | `true` | 内容页提示行，以及全屏页面板边框内的提示文字 |
| `page_lyrics` | `false` | 内容页上的两行歌词浮窗 |
| `page_lyrics_drag` | `true` | 歌词浮窗可用鼠标拖动 |
| `page_lyrics_snap` | `true` | 拖动结束后吸附到最近的边（左/右/上/下，另一轴保持自由；仅拖动开启时可改） |
| `page_lyrics_pos_x` | `1.0` | 歌词浮窗左上角的归一化横坐标（0=左，1=右） |
| `page_lyrics_pos_y` | `1.0` | 歌词浮窗左上角的归一化纵坐标（0=上，1=下） |
| `word_lyrics` | `true` | 有 YRC 时逐字着色 |
| `lyric_translation` | `false` | 显示歌词翻译 |
| `lyric_frame_divisor` | `2` | 歌词刷新相对 `ui_fps` 的倍率（1–8） |
| `small_window_display` | `true` | 终端过小时启用紧凑布局 |
| `home_more_recommend` | `false` | 首页在三块固定磁贴之外展开更多推荐 |
| `default_opening_title` | `""` | 替换登录页与加载页的 ASCII 标题，支持 `\n` |
| `audio_quality` | `exhigh` | `standard`、`higher`、`exhigh`、`lossless`、`hires`、`jyeffect`、`sky`、`dolby`、`jymaster`；非 VIP 账号会被限制到 `exhigh` |
| `download_audio_quality` | `exhigh` | 下载音质：与 `audio_quality` 同一套可选值，同样按 VIP 权限放开 |
| `download_path` | 未设置 | 下载目录（绝对路径，默认 `<系统音乐目录>/cnmplayer/`，没有音乐目录时回退 `~/Music/cnmplayer/`）；留空或填字面量 `Null`（不区分大小写）表示显式禁用下载 |
| `playback_memory` | `false` | 持久化并恢复队列、索引与循环模式 |
| `eq_bands_db` | 10 个 `0.0` | 均衡器各段增益（dB），在全屏 EQ 弹窗中调整 |
| `bar_number` | `auto` | `auto`、`16`、`32`、`48`、`64`、`80`、`96`（全屏频谱） |
| `bar_channels` | `mono` | `stereo`、`mono` |
| `bar_channel_reverse` | `false` | 左右声道反画（全屏频谱） |
| `bars_gap` | `false` | 频谱条之间留出间隔 |
| `ui_fps` | `60` | 正整数，控制主程序与全屏页（包括空闲时）的 UI 提交上限；无变化的帧可跳过，实际 FPS 取决于终端速度与处理开销 |
| `cache.path` | 未设置 | 缓存目录覆盖（默认用系统缓存目录） |
| `cache.clean_strategy` | `both` | `size`、`age`、`both` |
| `cache.max_size_mb` | `500` | LRU 阶段的容量上限 |
| `cache.max_age_days` | `7` | 按时间清理阶段的有效期 |
| `cache.clean_on_startup` | `true` | 启动时执行清理 |
| `keybind_*` | 见下文 | 22 个可重绑快捷键 |

清理按「先按时间、再按容量 LRU」两遍执行，且只统计目录下直属的文件。

## 快捷键

### 可重绑

绑定写法：可选 `Ctrl` / `Alt` / `Shift`，然后是键名（`Esc`、`Enter`、`Space`、`Tab`、`BackTab`、方向键、`Home`、`End`、`PageUp`/`PgUp`、`PageDown`/`PgDn`、`Insert`、`Delete`、`Backspace`、`Plus`、`F1`–`F12`）或单个字符。
与其它槽位冲突的绑定会被拒绝，在按键绑定弹窗内按 `Ctrl+Alt+R` 可恢复全部默认值。

| 配置项 | 默认值 | 作用 |
| --- | --- | --- |
| `keybind_search_box` | `Ctrl+S` | 打开搜索框 |
| `keybind_fullscreen` | `Ctrl+F` | 进入全屏播放页，或在全屏页内返回主程序 |
| `keybind_settings` | `T` | 打开设置弹窗（也可关闭） |
| `keybind_sidebar` | `P` | 切换首页侧边栏 / 全屏页歌单浮层 |
| `keybind_quit` | `Q` | 退出 |
| `keybind_page_up` | `pageUP` | 向上翻页（搜索页、歌单页） |
| `keybind_page_down` | `pageDown` | 向下翻页（搜索页、歌单页） |
| `keybind_prev` | `Alt+Left` | 上一首 |
| `keybind_next` | `Alt+Right` | 下一首 |
| `keybind_toggle_play_pause` | `Alt+Space` | 播放 / 暂停 |
| `keybind_toggle_mode` | `Alt+M` | 切换循环模式（主程序） |
| `keybind_fullscreen_prev` | `Left` | 上一首（仅全屏页） |
| `keybind_fullscreen_next` | `Right` | 下一首（仅全屏页） |
| `keybind_fullscreen_toggle_play_pause` | `Space` | 播放 / 暂停（仅全屏页） |
| `keybind_fullscreen_toggle_mode` | `M` | 切换循环模式（仅全屏页） |
| `keybind_fullscreen_eq` | `E` | 打开均衡器弹窗（仅全屏页） |
| `keybind_fullscreen_eq_reset` | `Alt+R` | 重置均衡器（仅全屏页） |
| `keybind_toggle_like_fullscreen` | `L` | 收藏 / 取消收藏（仅全屏页） |
| `keybind_toggle_like_collapsed` | `Alt+L` | 折叠播放栏的收藏 / 取消收藏 |
| `keybind_small_window_toggle` | `Alt+X` | 在扁窗的两个面板之间切换 |
| `keybind_download` | `Ctrl+Alt+D` | 下载当前聚焦的单曲（再按一次取消） |
| `keybind_download_fullscreen` | `Ctrl+D` | 下载当前播放的歌曲（仅全屏页，再按一次取消） |

仅全屏页生效的槽位在主程序里是空操作：那些按键会继续按页面导航处理。

### 固定快捷键

- `Esc`：关闭当前浮层，或从当前页面返回
- `Ctrl+C`：任意状态下退出
- `Ctrl+K`：打开按键绑定列表；列表内再次按下时，两端均直接关闭整窗，不回到设置根页。`Esc` 仍保留返回上级的行为。
- `Ctrl+Up` / `Ctrl+Down`：侧边栏展开时切换歌单分区（用户创建 / 用户收藏）
- `Ctrl+Alt+R`：恢复默认快捷键（在按键绑定弹窗内）
- `F1` / `F2` / `F3`：登录方式（二维码 / 账号 / 手机号）

### 各页面

登录页：

- `F1` 刷新二维码，`F2` 账号登录，`F3` 手机号登录
- `Tab` / `Down` 下一个字段，`Shift+Tab` / `Up` 上一个字段，`Enter` 确认或提交
- 用户名 / 密码输入框未聚焦时 `Q` 退出

搜索框：

- `Enter` 执行搜索，`Esc` / `Ctrl+S` 关闭
- `Home` / `End` / `Left` / `Right` / `Backspace` / `Delete` 编辑内容；点击可定位光标

搜索页、歌单页、作者页：

- `Enter` 打开或播放当前项，`Esc` 或 `Left` 返回
- `Tab` / `Down` 下一项，`Shift+Tab` / `Up` 上一项
- `PageUp` / `PageDown` 整页移动（搜索页与歌单页）

设置弹窗：

- `Up` / `Down` / `Tab` / `Shift+Tab` 移动，`Left` / `Right` / `Enter` 修改取值，`Esc` 逐级返回
- 鼠标：滚轮移动选中行，单击聚焦、双击执行；「歌词浮窗」子页为单击直接改值
- 按键绑定页：`Enter` 开始重绑，等待输入时按 `Esc` 取消；`Ctrl+K` 关闭整窗并取消待输入的重绑，不修改原快捷键

全屏播放页：

- 终端宽度小于 50 列时，主程序不响应打开全屏的快捷键
- `P` 打开歌单浮层，`Up` / `Down` 选择，`Enter` 播放，`Esc` 关闭
- `T` 打开设置弹窗，`Ctrl+K` 打开按键绑定列表，`About` 在设置弹窗内；这些弹窗均立即打开/关闭，暂停播放时同样生效
- `E` 打开均衡器弹窗；方向键选择与调整，`Alt+R` 重置，`Esc` / `E` 关闭
- `Up` / `Down` 调整音量，`Left` / `Right` 切歌，`Space` 播放或暂停，`M` 切换循环模式，`L` 收藏当前歌曲
- `Ctrl+F` 或 `Esc` 返回主程序；鼠标可点击控制按钮、进度条、音量条（点击或按住拖动）、爱心与歌单行；点作者名/专辑名会退出全屏页，并在主程序里打开对应的作者页/专辑页（多作者按各自名字分段，点谁的名字进谁）；弹窗打开时滚轮切换聚焦行，条目单击聚焦、双击执行（EQ 弹窗点击即设定该段增益）
- 若开启了 `small_window_display` 且终端降到 50 列或 12 行以下，全屏页会自动返回主程序
- 全屏进出转场像抽屉一样从底部整体上拉（退出时整体下滑）；进入转场结束后，鼠标控件立即可用

## 注意事项

- 没有命令行参数。可用的环境变量是 `CNMPLAYER_ASSET_DIR`（资产根目录）与 `COLORTERM` / `TERM`（颜色能力探测）。
- 预编译产物与 AUR 包只提供 Linux `amd64` 与 `aarch64`；MPRIS 同样仅 Linux 可用。
- API 回包与封面下载从响应头到完整 body 共用 30 秒 deadline；音频 body 不设整首下载总时限，仍可协作式取消。
- 封面不会回退到 ASCII 字符画：加载阶段最多显示临时 halfblock 预览，无封面或失败时保持空白。

## 技术栈

- Rust 2024
- TUI：ratatui + crossterm
- 异步与网络：compio + cyper
- 网易云 API：`ncm-api`（由 [ncm-api-rs](https://github.com/imsyy/ncm-api-rs) 以 path 依赖形式 vendored 进 `ncm-api-rs/`）
- 播放：rodio + symphonia（mp3 / flac），后端为 PipeWire
- 元数据、封面与下载标签：image + qrcode + lofty
- 图像渲染：ratatui-image + chafa
- 可视化：内部 Cava Rust 频谱移植（realfft），由共享播放 PCM 抽头驱动
- Linux 媒体控制：mpris-server
- 全屏播放：内置 `src/tmplayer/` UI，使用主程序播放链路与共享配置

## 开发

```bash
cargo run                              # 开发构建
cargo build --release                  # release 构建
cargo test --workspace --all-targets   # 根 crate 与 vendored crate 测试
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

CI（`ci.yml`）会在 Rust 1.95 与 stable 上分别执行默认特性和 `--no-default-features` 的 check/test，并对主程序与 vendored crate 执行 fmt、clippy 门禁，外加 vendored crate 的测试。
发版（`release.yml`）会在发布前执行根 crate 与 vendored crate 测试，随 `x86_64` 与 `aarch64` 压缩包发布 `SHA256SUMS`，并维护 AUR 包。

## 相关项目

- [ncm-api-rs](https://github.com/imsyy/ncm-api-rs)：vendored 在 `ncm-api-rs/` 目录中的网易云音乐 API 客户端

## 免责声明

完整免责声明——非官方客户端、音乐版权、缓存与下载仅限个人使用，以及账号风险与责任条款——见 [DISCLAIMER_zh.md](DISCLAIMER_zh.md)。

## 许可证

CNMPlayer 采用 [AGPL-3.0-only](LICENSE) 许可证。

vendored 代码、改写算法与 FFT 依赖的第三方归属和许可证声明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)，预编译发布包也包含该文件。

标准引用元数据和上游依赖请查看 [CITATION.cff](CITATION.cff)。

---
## Star History

[![Star History Chart](https://api.star-history.com/image?repos=professor-lee/CNMPlayer&type=date&legend=top-left)](https://www.star-history.com/?repos=professor-lee%2FCNMPlayer&type=date&legend=top-left)
