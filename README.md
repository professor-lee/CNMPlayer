<h1 align="center"><img src="logo.svg"/></h1>

<p align="center">
	<a href="README.md">English</a>
	&nbsp;&nbsp;&nbsp;|&nbsp;&nbsp;&nbsp;
	<a href="README_zh.md">简体中文</a>
</p>

<p align="center" style="color:gray;">
	A Rust TUI client for NetEase Cloud Music, with an embedded fullscreen playback page.
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

## Project Overview

CNMPlayer (Customized Netease Music Player) is a terminal NetEase Cloud Music client.
It supports QR code, account (username/email), and phone verification-code login; automatically restores the last session on startup;
browses home recommendations, playlist/album results, artist pages, and search pages; and streams songs in the terminal with local caching.
When you switch into fullscreen playback, CNMPlayer hands control to the embedded TMPlayer fullscreen page.

## Main Features

- QR code, account (username/email), and phone verification-code login
- Automatic session restore on startup
- Home recommendations, playlist pages, artist pages, and search pages; `@album` search results reuse the playlist-page layout
- Search suffixes: `@single`, `@album`, `@list`, `@author`, and the `@artist` alias; an empty `@author` query lists followed artists
- Streaming playback with a local audio cache
- Playback queue memory and local playback position restore
- VIP-aware audio quality clamping
- Page lyrics overlay on content pages
- Theme switching, language switching, transparent background, hint toggles, and configurable keybinds
- Bars / oscilloscope visualization; if `cava` is not installed, visualization is automatically disabled
- Embedded TMPlayer fullscreen page; the main UI's `cava` is paused/resumed when entering/leaving fullscreen
- Linux MPRIS sync
- Audio cache cleanup controls

## Notes

- Current image protocol only implements `off` / `halfblocks`; legacy `auto`, `sixel`, `kitty`, and `iterm2` values are migrated to `halfblocks`
- There is no dedicated album page; album search results are shown with the playlist-page layout
- `Esc`, `Ctrl+K`, and `Ctrl+Up/Down` are fixed shortcuts and cannot be rebound
- The app fills in missing config fields on startup and rewrites `config/default.toml` when needed

## Tech Stack

- Rust 2024
- TUI: ratatui + crossterm
- Networking: compio + cyper + ncm-api-rs
- Playback: rodio + symphonia + cpal
- Metadata and artwork: lofty + image + qrcode
- Image rendering: ratatui-image + chafa
- Visualization: external `cava`
- Fullscreen playback integration: TMPlayer
- Linux media control: MPRIS

## Development and Run

### Terminal Font

The UI uses icon glyphs in several places. A Nerd Font is strongly recommended; otherwise some icons may render as missing glyph boxes.

### Requirements (Linux)

Install the build dependencies provided by your distribution. On Debian/Ubuntu, this is usually enough:

```bash
sudo apt update
sudo apt install -y build-essential cmake pkg-config libasound2-dev libdbus-1-dev libchromaprint-dev
```

### Spectrum Visualization (`cava`)

CNMPlayer looks for an external `cava` binary for the live spectrum visualizer.
If `cava` is not available, the app still runs, but the bars and oscilloscope visualizers are automatically disabled.

The executable lookup order is:

1. `TMPLAYER_CAVA`
2. `<executable dir>/cava`
3. `<executable dir>/third_party/cava/cava`
4. `<current working directory>/third_party/cava/cava`
5. `cava` in `PATH`

### Run

For development:

```bash
cargo run
```

### Release build

```bash
cargo build --release
./target/release/cnmplayer
```

### First Run and Asset Root

On first run, the app creates its asset directory under your OS config directory; on Linux this is usually `~/.config/cnmplayer`.
If `CNMPLAYER_ASSET_DIR` is set, that directory becomes the asset root instead.
The app keeps `config/`, `themes/`, and `auth/` under that root.

After the first run you will see:

- `config/default.toml`
- `themes/*.toml`
- `auth/session.toml`

Audio cache files are stored under your OS cache directory unless you set `cache.path` in `config/default.toml`.

## Configuration

- `config/default.toml`: application settings, playback settings, keybinds, and cache policy
- `themes/*.toml`: theme definitions
- `auth/session.toml`: persisted login cookie
- Cache root: OS cache directory by default, or `cache.path` if you set one

The app fills in missing config fields on startup and rewrites `config/default.toml` when needed. Legacy `graphics_protocol` values `auto`, `sixel`, `kitty`, and `iterm2` are migrated to `halfblocks`.

Important settings in `config/default.toml`:

- Runtime: `ui_fps`, `spectrum_hz`, `mpris_poll_ms`
- Interface: `theme`, `language`, `transparent_background`, `show_hints`, `home_more_recommend`, `album_border`
- Login banner: `default_opening_title` (supports `\n` line breaks)
- Image and visualization: `graphics_protocol`, `visualize`, `super_smooth_bar`, `bars_gap`, `bar_number`, `bar_channels`, `bar_channel_reverse`, `kitty_cover_scale_percent`
- Playback behavior: `audio_quality`, `playback_memory`, `resume_last_position`, `eq_bands_db`
- Lyrics and recognition: `page_lyrics`, `lyrics_cover_fetch`, `lyrics_cover_download`, `audio_fingerprint`, `acoustid_api_key`
- Keybinds: `keybind_*` (see below; can be rebound in Settings)
- Cache policy: `cache.path`, `cache.clean_strategy`, `cache.max_size_mb`, `cache.max_age_days`, `cache.clean_on_startup`

Additional notes:

- `theme` can be `system`, `latte`, `frappe`, `macchiato`, or `mocha`; the default is `frappe`
- `graphics_protocol` currently only implements `off` / `halfblocks`
- `visualize` supports `off`, `bars`, and `oscilloscope`; if `cava` is unavailable it falls back to `off`
- `cache.clean_strategy` supports `size`, `age`, and `both`
- `audio_quality` supports `standard`, `higher`, `exhigh`, `lossless`, `hires`, `jyeffect`, `sky`, `dolby`, and `jymaster`
- If the current account does not have VIP access, CNMPlayer clamps the quality to the free range

## Keyboard Shortcuts

Configurable shortcuts (default bindings):

- `Ctrl+S`: open the search box
- `Ctrl+F`: open / return to fullscreen playback
- `T`: open settings
- `P`: toggle the sidebar
- `Q`: quit the host app
- `Alt+Space`: toggle play/pause
- `Alt+Left`: previous track
- `Alt+Right`: next track
- `Alt+M`: toggle repeat mode
- `Left`: fullscreen previous track
- `Right`: fullscreen next track
- `Space`: fullscreen play/pause
- `M`: toggle fullscreen playback mode
- `E`: toggle fullscreen EQ
- `Alt+R`: reset fullscreen EQ
- `L`: toggle like/unlike in fullscreen
- `Alt+L`: toggle like/unlike in the collapsed player bar

Fixed shortcuts:

- `Esc`: close overlays or go back from the current page
- `Ctrl+Up` / `Ctrl+Down`: switch sidebar playlist section (Created / Collected) when the sidebar is expanded
- `Ctrl+K`: open help

Login page:

- `F1`: QR login
- `F2`: account login (username / email)
- `F3`: phone login
- `Q`: quit the app
- `Tab` / `Up` / `Down`: switch focus
- `Enter`: confirm or submit

Search box:

- `Enter`: run the search
- `Esc` / `Ctrl+S`: close the search box
- `Backspace`: delete text
- Arrow keys: move the cursor

Search, playlist, and author pages:

- `Enter`: open or play the focused item
- `Esc` or `Left`: go back
- `Tab` / `Down`: move to the next item
- `Shift+Tab` / `Up`: move to the previous item

Settings keybind page:

- `Enter`: start rebinding the selected shortcut
- `Ctrl+Alt+R`: reset keybinds to defaults
- `Esc`: return

## Related Projects

- [TMPlayer](https://github.com/professor-lee/TMPlayer): fullscreen playback UI used by CNMPlayer
- [ncm-api-rs](https://github.com/imsyy/ncm-api-rs): NetEase Cloud Music API client used by CNMPlayer

## License

CNMPlayer is licensed under [AGPL-3.0-only](LICENSE).

Third-party attributions and license notices for vendored code are documented in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

See [CITATION.cff](CITATION.cff) for the standard citation metadata and upstream references.

---
## Star History

[![Star History Chart](https://api.star-history.com/image?repos=professor-lee/CNMPlayer&type=date&legend=top-left)](https://www.star-history.com/?repos=professor-lee%2FCNMPlayer&type=date&legend=top-left)