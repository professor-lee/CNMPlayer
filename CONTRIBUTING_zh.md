<p align="center">
	<a href="CONTRIBUTING.md">English</a>
	&nbsp;&nbsp;&nbsp;|&nbsp;&nbsp;&nbsp;
	<a href="CONTRIBUTING_zh.md">简体中文</a>
</p>

# 为 CNMPlayer 贡献代码

CNMPlayer 是一个运行在终端中的网易云音乐客户端（Rust + TUI）。本文档说明分支与 PR 规则、开发环境、仓库布局、格式与 CI 约定，以及终端界面特有的额外要求。

## 分支与 PR 规则

- **所有 PR 一律向 `develop` 分支发起。** `develop` 是集成分支，不要直接向 `main` 发起 PR。
- **`main` 是发布分支，仅由仓库作者维护**：由作者本人从 `develop` 向 `main` 发起 PR，请不要自行向 `main` 提交改动。
- **一个 PR 只处理一个主题。** 面向 `develop` 的每个 PR 只能处理一个修改主题；有多个互不相关的修改时，请拆成多个 PR —— 混合多个主题的 PR 将被拒绝。重大贡献除外，由作者判断。

## 开发环境

Rust 1.95 或更新版本；仓库在 `rust-toolchain.toml` 中固定 1.95.0。根 crate（`cnmplayer`）使用 edition 2024，vendored 的 `ncm-api` crate 使用 edition 2021。

锁定依赖需要 Rust 1.95：`compio-driver 0.12.4` 使用了在 1.95 稳定的 [`std::cfg_select!`](https://doc.rust-lang.org/stable/std/macro.cfg_select.html)。依赖 manifest 声明曾提示 1.90，但真实 1.90 构建在 `compio-buf 0.8.3` 的 `MaybeUninit` slice API 处失败，1.93 构建又在 `cfg_select!` 处失败。单看 edition 2024 只需要 1.85；edition 与不完整的 manifest 声明都不能代替真实依赖 MSRV 验证。根 crate 与 vendored crate 共用一个 workspace lockfile。

系统构建依赖 —— 与 CI 在 `ubuntu-24.04` 上安装的列表一致：

```bash
# Debian / Ubuntu
sudo apt update
sudo apt install -y build-essential cmake pkg-config \
  libasound2-dev libchafa-dev libpipewire-0.3-dev libssl-dev libglib2.0-dev libclang-dev
```

```bash
# Arch Linux
sudo pacman -S --needed base-devel cmake pkg-config alsa-lib chafa pipewire openssl glib2 clang
```

- `libchafa-dev` 要求 chafa ≥ 1.8.0 —— 图像渲染通过 `pkg-config` 探测它；
- `libclang-dev` 与 `libpipewire-0.3-dev` 是 PipeWire 音频后端在构建期生成绑定所需；
- `libasound2-dev` / `alsa-lib` **只在构建期需要**：`cpal` 在 Linux 上无条件编译其 ALSA 后端，缺了这个开发包就构建不出来；但播放走 PipeWire，ALSA 路径不属于受支持的运行配置。

运行要求：Linux 上的 PipeWire 音频（ALSA 后端已弃用，不要按它测试或写文档），外加运行时的 chafa 共享库。频谱可视化全部为内部实现（`bars` 是 Cava 算法的 Rust 移植），不涉及外部 `cava` 可执行文件。播放和导航图标要求安装 Nerd Font，程序固定使用 Nerd Font 字形。

常用命令：

```bash
cargo run                              # 开发构建
cargo build --release                  # release 构建
cargo test --workspace --all-targets   # 根 crate 与 vendored crate 测试
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

开发时有用的环境变量：`CNMPLAYER_ASSET_DIR`（资产根目录，默认 `~/.config/cnmplayer`）。

Cargo feature：`default = ["easter-egg"]` —— About 弹窗里的形象彩蛋，用 `--no-default-features` 可剔除。

## 仓库布局

| 路径 | 内容 |
| --- | --- |
| `src/main.rs` | 入口：终端初始化、资产根目录，以及交给应用程序。 |
| `src/app/` | 应用核心。`mod.rs` 是装配点与顶层协调者：`App` 结构体（共享状态、各子控制器、命中区），以及驱动它们的事件处理。领域类型、请求与行为各自归对应职责模块 —— `ApiState` 与封面抓取在 `api.rs`，响应结构辅助函数在 `response.rs`，搜索在 `search_controller.rs`，browse/download/input/playback/settings/startup 各有对应的 `*_controller.rs`，另有 player、streaming、download 与 MPRIS 模块。 |
| `src/ui/` | 主程序 UI 面板：登录、首页、歌单、作者、搜索、设置、播放栏、歌词、加载、小窗口与主题模块。 |
| `src/data/` | 共享 `Config`，以及资产、原子持久化、会话、播放/私人漫游状态与主题加载。 |
| `src/render/` | 封面与图形渲染，以及彩蛋形象的帧数据。 |
| `src/tmplayer/` | 内置全屏播放 UI、渲染器与音频可视化辅助模块；播放由主程序负责，配置使用共享 `Config`。 |
| `ncm-api-rs/` | 以 workspace path 依赖形式 vendored 的 `ncm-api` crate，来自 [imsyy/ncm-api-rs](https://github.com/imsyy/ncm-api-rs)；自带 `rustfmt.toml`、`clippy.toml` 与 `docs/API.md`。 |
| `config/default.toml` | 默认配置模板。启动时程序会据此在资产根目录下写入并修复 `config/default.toml`。 |
| `themes/` | 内置 20 款 TOML 配色主题（Catppuccin 各变体、ayu、everforest、monokai、nord、rose pine、solarized、tomorrow、zenburn、zinc 与 `system`；默认 `frappe`）。 |
| `about/` | About 弹窗的内容 —— 链接、QQ 群、点阵图案。 |
| `assets/`、`tools/` | 彩蛋形象源图，以及重新生成其点阵帧的脚本。 |
| `.github/` | `workflows/ci.yml`、`workflows/release.yml` 与 `scripts/aur_sync.sh`。 |
| 根目录文档 | `README.md` / `README_zh.md`（中英两份并行维护）、`DISCLAIMER.md` / `DISCLAIMER_zh.md`、`CITATION.cff`、`THIRD_PARTY_NOTICES.md`、`LICENSE`（AGPL-3.0-only）。 |

## 模块维护规则

`src/app/` 按职责拆分，`mod.rs` 只负责装配与顶层协调，不承载领域实现。新增代码请沿用同一形状：

- **显式导入。** 用到什么就导入什么；不要加 glob 导入，也不要为了省事经由上级模块转发条目 —— 调用方直接导入定义该条目的模块。
- **直接引用所属模块。** 写 `crate::app::search_controller::SearchController`，而不是 `crate::app::SearchController`：`mod.rs` 不是门面。领域类型、请求与行为归对应职责模块，所以就写在那里，而不是写进 `mod.rs`。
- **可见性取最小。** 只有确实被其他顶层模块（`src/ui/`、`src/render/`、`src/data/`、`src/tmplayer/`、`src/main.rs`）共用的接口才用 `pub(crate)`；`src/app/` 内部兄弟模块之间共用的用 `pub(super)`；其余保持模块私有，通过所属类型的方法访问。
- **搬动代码时不留兼容层。** 在同一次改动里更新所有调用方并删掉旧路径；靠 re-export 让旧路径继续编译的写法不予接受。

## 格式与 CI

格式化工具是 rustfmt，仓库自带配置：

- 根目录 `.rustfmt.toml`：`max_width = 100`、`edition = "2024"`；
- `ncm-api-rs/rustfmt.toml`：`edition = "2021"`、`max_width = 100`、`use_field_init_shorthand = true` —— 动过 vendored crate 时一并执行 `cargo fmt --manifest-path ncm-api-rs/Cargo.toml`。

Clippy：`cargo clippy --all-targets` 只是本地快速检查，下面的门禁会带上 `-D warnings`；`ncm-api-rs/clippy.toml` 把 `too-many-arguments-threshold` 放宽到 8、`type-complexity-threshold` 放宽到 300。

CI（`.github/workflows/ci.yml`）在面向 `main` / `develop` 的 PR 以及推送到 `develop` 时触发。所有 job 都跑在 `ubuntu-24.04` 上；`build-and-test` 与 `quality` 安装上面列出的系统依赖：

- `build-and-test`：以四组矩阵运行 `cargo check` 与 `cargo test`（`--workspace --locked --all-targets`）—— Rust 1.95.0 与 stable，各自再分默认特性与 `--no-default-features`。
- `quality`：在 stable 上并装好 `rustfmt`、`clippy` 组件后运行 `cargo fmt --all -- --check` 与 `cargo fmt --manifest-path ncm-api-rs/Cargo.toml -- --check`，随后是三次带 `-D warnings` 的 clippy —— 根 crate 的 `--all-features`、根 crate 的 `--no-default-features`，以及经 manifest 指向 vendored crate。
- `vendored-tests`：`cargo test --manifest-path ncm-api-rs/Cargo.toml --locked --all-targets`。

对应的本地命令（按 job 分组）：

```bash
# build-and-test：默认特性与 --no-default-features，每种工具链各跑一遍。
# 直接 `cargo` 已按 pin 选中 1.95.0；stable 那一半用 `cargo +stable`。
cargo check --workspace --locked --all-targets
cargo test --workspace --locked --all-targets
cargo check --workspace --locked --all-targets --no-default-features
cargo test --workspace --locked --all-targets --no-default-features

# quality（仅 stable）
cargo +stable fmt --all -- --check
cargo +stable fmt --manifest-path ncm-api-rs/Cargo.toml -- --check
cargo +stable clippy --locked --all-targets --all-features -- -D warnings
cargo +stable clippy --locked --all-targets --no-default-features -- -D warnings
cargo +stable clippy --manifest-path ncm-api-rs/Cargo.toml --locked --all-targets -- -D warnings

# vendored-tests
cargo test --manifest-path ncm-api-rs/Cargo.toml --locked --all-targets
```

随手一条 `cargo clippy` 不等同于门禁：`quality` 额外要求 `-D warnings`、覆盖两种特性组合，并单独对 vendored crate 执行 clippy，因此本地单独跑通并不代表该 job 会通过。

版本号与打包不属于普通 PR 的范围：发布由 `v*` tag 触发，tag 必须与 `Cargo.toml` 里的版本号一致；`release.yml` 会在发布前测试 workspace 两个 crate，再发布 amd64/aarch64 压缩包、`SHA256SUMS` 与 AUR 包。

## TUI 专属要求

终端界面不能只满足于「在我机器上是好的」。

- **键盘是主要交互方式。** 设计以键盘操作优先：任何功能都必须仅靠键盘就能到达并操作，两者冲突时以键盘路径为准。鼠标交互只是叠加在上层的附加功能，用于非必要功能，绝不能成为到达某个功能的唯一途径。
- **基本功能必须能在纯 TTY 下可用。** 以 `KMSCON` 为基准环境：没有模拟器专属转义序列，鼠标上报甚至可能完全不可用；必须安装 Nerd Font 才能正确显示图标。登录、浏览、播放控制与退出都必须在那里保持可用。
- **渲染类改动要附渲染证据。** 任何改变「画出来的东西」的改动 —— 布局、动画、封面、可视化、配色 —— 都要在 PR 里附截图或简短录屏。
- **说明你的运行环境**：终端模拟器（名称与版本）、字体、终端尺寸与 `TERM` 值。图标集固定使用 Nerd Font，不提供运行时字体能力探测；涉及输入时还要尝试 `KMSCON` 风格的纯 TTY。
- **覆盖小窗口阈值。** 主程序内容页低于 README「小窗口模式」一节给出的阈值时会切换为扁平布局；请在边界尺寸上验证，包括 `Terminal too small` 的情况。
- **改动到的控件要实测键盘路径，有鼠标目标的一并实测鼠标路径**：折叠播放栏的上一首 / 播放暂停 / 下一首、收藏与循环模式按钮，以及进度条都可点击；侧边栏支持滚轮、单击与双击（400 ms 判定窗口）。
- **新增或修改界面文案时，中英两种界面语言都要检查**（`language = "zh"` / `"en"`）。
- **可视化全部为内部实现。** `bars` 是 Cava 算法的 Rust 移植，不需要外部 `cava` 可执行文件；改动频谱或示波器时，请在播放、暂停与切歌后都实测一遍。
