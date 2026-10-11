<p align="center">
	<a href="CONTRIBUTING.md">English</a>
	&nbsp;&nbsp;&nbsp;|&nbsp;&nbsp;&nbsp;
	<a href="CONTRIBUTING_zh.md">简体中文</a>
</p>

# Contributing to CNMPlayer

CNMPlayer is a Rust TUI client for NetEase Cloud Music. This document covers the
branch and pull-request rules, the development environment, the repository
layout, the formatting and CI conventions, and the extra requirements that apply
to a terminal UI.

## Branch and pull request rules

- **Open pull requests against `develop`.** `develop` is the integration branch.
  Never open a pull request directly against `main`.
- **`main` is the release branch and is maintained by the repository owner
  only.** The maintainer opens pull requests from `develop` into `main`; do not
  propose changes to `main` yourself.
- **One PR, one topic.** Each pull request against `develop` must deal with a
  single subject. If you have several unrelated changes, open several pull
  requests — a PR that mixes topics will be rejected. Major contributions are
  the exception, at the maintainer's discretion.

## Development environment

Rust 1.95 or newer; the repository pins 1.95.0 in `rust-toolchain.toml`. The root crate (`cnmplayer`) uses edition 2024; the vendored `ncm-api` crate uses edition 2021.

The locked dependencies require Rust 1.95: `compio-driver 0.12.4` uses [`std::cfg_select!`](https://doc.rust-lang.org/stable/std/macro.cfg_select.html), stabilized in 1.95. Dependency manifest declarations suggested 1.90, but an actual 1.90 build failed at `compio-buf 0.8.3`'s `MaybeUninit` slice API and a 1.93 build failed at `cfg_select!`. Edition 2024 alone would only require 1.85; neither edition nor incomplete manifest declarations establish the real dependency MSRV. The root and vendored crate share one workspace lockfile.

System build dependencies — the same list CI installs on `ubuntu-24.04`:

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

- `libchafa-dev` must be chafa ≥ 1.8.0 — the image renderer probes it through
  `pkg-config`;
- `libclang-dev` and `libpipewire-0.3-dev` are needed because the PipeWire
  audio backend generates bindings at build time;
- `libasound2-dev` / `alsa-lib` is needed at **build time** only: `cpal`
  compiles its ALSA backend unconditionally on Linux, so the develop package
  must be installed to build, but playback goes through PipeWire and the ALSA
  path is not a supported runtime configuration.

Runtime: PipeWire for audio — the ALSA backend is deprecated, do not test or document it — plus the chafa shared library. The spectrum visualizers are internal (`bars` is a Rust port of Cava's algorithm), so no external `cava` binary is involved. A Nerd Font is required for playback and navigation icons; the application uses Nerd glyphs directly.

Commands:

```bash
cargo run                              # development build
cargo build --release                  # release build
cargo test --workspace --all-targets   # root and vendored crate tests
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Useful environment variables while developing: `CNMPLAYER_ASSET_DIR` (asset root,
`~/.config/cnmplayer` by default).

Cargo features: `default = ["easter-egg"]` — the mascot in the About modal. Build
with `--no-default-features` to drop it.

## Repository layout

| Path | Contents |
| --- | --- |
| `src/main.rs` | Entry point: terminal setup, asset root, and hand-off to the application. |
| `src/app/` | Application core: shared state in `mod.rs`, API/streaming/player/download services, MPRIS bridge, `SearchController` in `controllers.rs`, and focused browse/download/input/playback/settings/startup controllers. |
| `src/ui/` | Host UI panels: login, home, playlist, author, search, settings, player bar, lyrics, loading, small-window and theme modules. |
| `src/data/` | Shared `Config` plus assets, atomic persistence, sessions, playback/private-roam state, and theme loading. |
| `src/render/` | Cover and graphics rendering, plus the easter-egg mascot frames. |
| `src/tmplayer/` | Embedded fullscreen playback UI, its renderers and audio visualization helpers; playback is owned by the host and configuration uses the shared `Config`. |
| `ncm-api-rs/` | The `ncm-api` crate vendored from [imsyy/ncm-api-rs](https://github.com/imsyy/ncm-api-rs) as a workspace path dependency. It ships `rustfmt.toml`, `clippy.toml` and `docs/API.md`. |
| `config/default.toml` | The default configuration template. At startup the app writes and repairs `config/default.toml` under the asset root from it. |
| `themes/` | 20 built-in TOML color themes (Catppuccin variants, ayu, everforest, monokai, nord, rose pine, solarized, tomorrow, zenburn, zinc and `system`; default `frappe`). |
| `about/` | Content of the About modal — links, QQ group, braille art. |
| `assets/`, `tools/` | Mascot source image and the script that regenerates its braille frames. |
| `.github/` | `workflows/ci.yml`, `workflows/release.yml` and `scripts/aur_sync.sh`. |
| Root documents | `README.md` / `README_zh.md` (two language variants kept in parallel), `DISCLAIMER.md` / `DISCLAIMER_zh.md`, `CHANGELOG`-free release notes on the Releases page, `CITATION.cff`, `THIRD_PARTY_NOTICES.md`, `LICENSE` (AGPL-3.0-only). |

## Formatting and CI

Formatting is `rustfmt`; the repository pins its own configuration:

- root `.rustfmt.toml`: `max_width = 100`, `edition = "2024"`;
- `ncm-api-rs/rustfmt.toml`: `edition = "2021"`, `max_width = 100`,
  `use_field_init_shorthand = true` — run
  `cargo fmt --manifest-path ncm-api-rs/Cargo.toml` when you touch the vendored
  crate.

Linting is `cargo clippy --all-targets`; `ncm-api-rs/clippy.toml` relaxes
`too-many-arguments-threshold` to 8 and `type-complexity-threshold` to 300.

CI (`.github/workflows/ci.yml`) runs on pull requests targeting `main` / `develop` and on pushes to `develop`. It installs the system dependencies listed above on `ubuntu-24.04`, checks Rust 1.95 and stable with both default and `--no-default-features`, and runs fmt/clippy gates for the root and vendored crates.

The equivalent local commands are:

```bash
cargo check --workspace --locked --all-targets
cargo test --workspace --locked --all-targets
cargo fmt --all -- --check
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
```

Version bumps and packaging are not part of a normal pull request: releases are
triggered by a `v*` tag, the tag must equal the version in `Cargo.toml`, and
`release.yml` tests both workspace crates before publishing amd64/aarch64 tarballs,
their `SHA256SUMS`, and the AUR packages.


## TUI-specific requirements

A patch that "works on my machine" is not enough for a terminal UI.

- **The keyboard is the primary interface.** Design keyboard-first: every
  function must be reachable and operable with the keyboard alone, and the
  keyboard path is the reference behaviour when the two disagree. Mouse
  interaction is an optional layer on top of it — a convenience for
  non-essential features, never the only way to reach something.
- **Basic functionality must work in a plain TTY.** `KMSCON` is the reference environment: no emulator-only escape sequences, and mouse reporting may not be available at all. A Nerd Font must be installed for the icon set. Login, browsing, playback control and quitting must all stay usable.
- **Rendering changes need rendering evidence.** For anything that changes what
  is drawn — layout, animation, cover art, visualizers, colors — attach a
  screenshot or a short recording to the pull request.
- **State your environment**: terminal emulator (name and version), font, terminal size, and `TERM` value. The icon set is fixed to Nerd Font glyphs; there is no runtime font-capability probe. When you touch input handling, keys or keybinds, also try a bare `KMSCON`-style TTY.
- **Cover the small-window thresholds.** Host content pages switch to the flat
  layout below the thresholds documented in README → *Small window mode*; verify
  at the boundary sizes, including the `Terminal too small` case.
- **Exercise the keyboard path for the widgets you touch, plus the mouse path
  where one exists**: the collapsed player bar's previous / play-pause / next,
  like and repeat-mode buttons and its progress bar are clickable, and the
  sidebar supports the wheel, a single click and a double click (400 ms
  window).
- **Check both UI languages** (`language = "zh"` / `"en"`) when you add or change
  user-visible strings.
- **The visualizers are internal.** `bars` is a Rust port of Cava's algorithm and
  needs no external `cava` binary; when you touch the spectrum or the
  oscilloscope, exercise playing, paused and post-track-change states.
