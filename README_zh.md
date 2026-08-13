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
它支持二维码、账号（用户名/邮箱）和手机号验证码登录，启动时会自动恢复上次会话；
可以浏览首页推荐、歌单/专辑结果、作者页和搜索页，并把歌曲流式播放到终端中，同时缓存音频到本地。
切换到全屏播放时，CNMPlayer 会交给内置的 TMPlayer 全屏播放页。

## 主要功能

- 二维码、账号（用户名/邮箱）和手机号验证码登录
- 启动时自动恢复上次登录会话
- 首页推荐、歌单页、作者页和搜索页；`@album` 搜索结果会以歌单页样式展示
- 搜索后缀支持 `@single`、`@album`、`@list`、`@author`，以及 `@artist` 别名；空查询的 `@author` 会列出已关注作者
- 流式播放，并带本地音频缓存
- 支持播放队列记忆，以及本地播放位置恢复
- 支持按 VIP 权限自动裁剪的音质选择
- 内容页歌词浮层
- 主题切换、语言切换、透明背景、提示开关和可配置快捷键
- 频谱条 / 示波器可视化；如果系统里没有 `cava`，可视化会自动关闭
- 内置 TMPlayer 全屏播放页，并会在进出全屏时自动暂停 / 恢复主界面的 `cava`
- Linux 下支持 MPRIS 媒体控制同步
- 音频缓存清理控制

## 注意事项

- 当前图像协议只实现 `off` / `halfblocks`；旧的 `auto`、`sixel`、`kitty`、`iterm2` 会自动迁移到 `halfblocks`
- 目前没有独立的“专辑页”；专辑搜索结果会以歌单页样式展示
- `Esc`、`Ctrl+K` 和 `Ctrl+Up/Down` 属于固定快捷键，不在可重绑项内
- 应用启动后会自动补齐缺失配置字段，并在需要时重写 `config/default.toml`

## 技术栈

- Rust 2024
- TUI：ratatui + crossterm
- 网络：compio + cyper + ncm-api-rs
- 播放：rodio + symphonia + cpal
- 元数据与封面：lofty + image + qrcode
- 图像渲染：ratatui-image + chafa
- 可视化：外部 `cava`
- 全屏播放整合：TMPlayer
- Linux 媒体控制：MPRIS

## 开发与运行

### 终端字体

界面中有一些图标字形，强烈建议使用 Nerd Font；如果没有这类字体，部分图标可能显示为缺字方块。

### 依赖（Linux）

请安装发行版提供的构建依赖。以 Debian/Ubuntu 为例：

```bash
sudo apt update
sudo apt install -y build-essential cmake pkg-config libasound2-dev libdbus-1-dev
```

### 频谱可视化（`cava`）

CNMPlayer 会查找外部 `cava` 可执行文件来生成实时频谱可视化。
如果系统里没有 `cava`，程序仍然可以运行，但条形频谱和示波器会自动关闭。

可执行文件的查找顺序如下：

1. `TMPLAYER_CAVA`
2. `<可执行文件目录>/cava`
3. `<可执行文件目录>/third_party/cava/cava`
4. `<当前工作目录>/third_party/cava/cava`
5. `PATH` 里的 `cava`

### 运行

开发环境直接运行：

```bash
cargo run
```

### Release 构建

```bash
cargo build --release
./target/release/cnmplayer
```

### 首次运行与资源目录

首次运行时，程序会在系统配置目录下创建资产目录；Linux 上通常是 `~/.config/cnmplayer`。
如果设置了 `CNMPLAYER_ASSET_DIR`，则会改用该目录作为资产根目录。
程序会在这个根目录下维护 `config/`、`themes/` 和 `auth/` 子目录。

首次运行后你会看到：

- `config/default.toml`
- `themes/*.toml`
- `auth/session.toml`

音频缓存默认保存在系统缓存目录中；如果你在 `config/default.toml` 里设置了 `cache.path`，则会改用该目录。

## 配置

- `config/default.toml`：程序配置、播放配置、快捷键和缓存策略
- `themes/*.toml`：主题定义
- `auth/session.toml`：持久化登录 cookie
- 缓存根目录：默认使用系统缓存目录，也可以通过 `cache.path` 指定

程序启动后会自动补齐缺失配置字段，并在需要时重写 `config/default.toml`。旧版 `graphics_protocol` 的 `auto`、`sixel`、`kitty`、`iterm2` 值也会自动迁移为 `halfblocks`。

`config/default.toml` 里比较重要的配置项：

- 运行参数：`ui_fps`、`spectrum_hz`、`mpris_poll_ms`
- 外观：`theme`、`language`、`transparent_background`、`show_hints`、`home_more_recommend`、`album_border`
- 登录页：`default_opening_title`（支持 `\n` 换行）
- 图像与可视化：`graphics_protocol`、`visualize`、`super_smooth_bar`、`bars_gap`、`bar_number`、`bar_channels`、`bar_channel_reverse`、`kitty_cover_scale_percent`
- 播放行为：`audio_quality`、`playback_memory`、`resume_last_position`、`eq_bands_db`
- 歌词与识别：`page_lyrics`、`lyrics_cover_fetch`、`lyrics_cover_download`、`audio_fingerprint`、`acoustid_api_key`
- 快捷键：`keybind_*`（详见下文，可在设置页重绑）
- 缓存策略：`cache.path`、`cache.clean_strategy`、`cache.max_size_mb`、`cache.max_age_days`、`cache.clean_on_startup`

补充说明：

- `theme` 可选 `system`、`latte`、`frappe`、`macchiato`、`mocha`，默认 `frappe`
- `graphics_protocol` 当前只实现 `off` / `halfblocks`
- `visualize` 支持 `off`、`bars`、`oscilloscope`；没有 `cava` 时会自动回退到 `off`
- `cache.clean_strategy` 支持 `size`、`age`、`both`
- `audio_quality` 支持 `standard`、`higher`、`exhigh`、`lossless`、`hires`、`jyeffect`、`sky`、`dolby`、`jymaster`
- 如果当前账号没有 VIP 权限，程序会把音质限制到免费档位

## 快捷键

可重绑快捷键（默认值）：

- `Ctrl+S`：打开搜索框
- `Ctrl+F`：打开 / 返回全屏播放页
- `T`：打开设置
- `P`：切换侧边栏
- `Q`：退出主程序
- `Alt+Space`：播放 / 暂停
- `Alt+Left`：上一首
- `Alt+Right`：下一首
- `Alt+M`：切换循环模式
- `Left`：全屏上一首
- `Right`：全屏下一首
- `Space`：全屏播放 / 暂停
- `M`：切换全屏播放模式
- `E`：切换全屏 EQ
- `Alt+R`：重置全屏 EQ
- `L`：在全屏页切换收藏状态
- `Alt+L`：在折叠播放器栏切换收藏状态

固定快捷键：

- `Esc`：关闭浮层或返回当前页面
- `Ctrl+Up` / `Ctrl+Down`：在侧边栏展开时切换歌单分区（用户创建 / 用户收藏）
- `Ctrl+K`：打开帮助

登录页：

- `F1`：二维码登录
- `F2`：账号登录（用户名 / 邮箱）
- `F3`：手机号登录
- `Q`：退出程序
- `Tab` / `↑` / `↓`：切换焦点
- `Enter`：确认或提交

搜索框：

- `Enter`：执行搜索
- `Esc` / `Ctrl+S`：关闭搜索框
- `Backspace`：删除文本
- 方向键：移动光标

搜索页、歌单页、作者页：

- `Enter`：打开或播放当前项
- `Esc` 或 `Left`：返回
- `Tab` / `Down`：切到下一项
- `Shift+Tab` / `Up`：切到上一项

设置页的按键绑定：

- `Enter`：开始重绑当前快捷键
- `Ctrl+Alt+R`：恢复默认快捷键
- `Esc`：返回

## 相关项目

- [TMPlayer](https://github.com/professor-lee/TMPlayer)：CNMPlayer 使用的全屏播放页实现
- [ncm-api-rs](https://github.com/imsyy/ncm-api-rs)：CNMPlayer 使用的网易云音乐 API 客户端

## 许可证

CNMPlayer 采用 [AGPL-3.0-only](LICENSE) 许可证。

仓库内 vendored 代码的第三方归属与许可证声明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

标准引用元数据和上游依赖请查看 [CITATION.cff](CITATION.cff)。

---
## Star History

[![Star History Chart](https://api.star-history.com/image?repos=professor-lee/CNMPlayer&type=date&legend=top-left)](https://www.star-history.com/?repos=professor-lee%2FCNMPlayer&type=date&legend=top-left)