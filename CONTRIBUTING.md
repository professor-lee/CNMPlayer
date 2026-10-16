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
| `src/app/` | Application core. `mod.rs` is the assembly point and top-level coordinator: the `App` struct with its shared state, sub-controllers and hit regions, plus the event handling that drives them. Domain types, requests and behaviour live in the module that owns the responsibility — `ApiState` and cover fetching in `api.rs`, response-shape helpers in `response.rs`, search in `search_controller.rs`, the focused browse/download/input/playback/settings/startup `*_controller.rs` modules, and the player, streaming, download and MPRIS modules. |
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

## Module conventions

`src/app/` is split by responsibility, and `mod.rs` holds the assembly and the top-level coordination — not the domain pieces. Keep new code in the same shape:

- **Import explicitly.** Name the items you use; do not add glob imports, and do not re-export items through a parent module for convenience. A caller reaches an item by importing the module that defines it.
- **Reference the owning module.** Write `crate::app::search_controller::SearchController`, not `crate::app::SearchController`: `mod.rs` is not a facade. Domain types, requests and behaviour belong to the module that owns that responsibility, so add them there rather than to `mod.rs`.
- **Keep visibility minimal.** Use `pub(crate)` only for interfaces genuinely shared with another top-level module (`src/ui/`, `src/render/`, `src/data/`, `src/tmplayer/`, `src/main.rs`); use `pub(super)` for items shared between siblings inside `src/app/`; everything else stays private to its module and is reached through the owning type's methods.
- **Leave no compatibility shims when you move code.** Update every caller in the same change and delete the old path; a re-export that keeps a stale path compiling is not accepted.
- **Keep tests in sibling files.** Production Rust modules must not embed test modules, test-only helpers, constants, or impls. Put tests for `foo.rs` in `foo/tests.rs` (and tests for `mod.rs` in the sibling `tests.rs`); keep production modules free of `cfg(test)` implementations. Preserve private access through the narrowest module visibility; do not add production APIs only for tests.

## Formatting and CI

Formatting is `rustfmt`; the repository pins its own configuration:

- root `.rustfmt.toml`: `max_width = 100`, `edition = "2024"`;
- `ncm-api-rs/rustfmt.toml`: `edition = "2021"`, `max_width = 100`,
  `use_field_init_shorthand = true` — run
  `cargo fmt --manifest-path ncm-api-rs/Cargo.toml` when you touch the vendored
  crate.

Linting is `cargo clippy --all-targets` — a quick local check. The gates below
run it with `-D warnings`; `ncm-api-rs/clippy.toml` relaxes
`too-many-arguments-threshold` to 8 and `type-complexity-threshold` to 300.

CI (`.github/workflows/ci.yml`) runs on pull requests targeting `main` / `develop` and on pushes to `develop`. Every job runs on `ubuntu-24.04`; `build-and-test` and `quality` install the system dependencies listed above:

- `build-and-test` runs `cargo check` and `cargo test`
  (`--workspace --locked --all-targets`) across a four-way matrix: Rust 1.95.0
  and stable, each with default features and with `--no-default-features`.
- `quality` runs on stable with the `rustfmt` and `clippy` components:
  `cargo fmt --all -- --check` and
  `cargo fmt --manifest-path ncm-api-rs/Cargo.toml -- --check`, then three
  clippy runs with `-D warnings` — the root crate with `--all-features`, the
  root crate with `--no-default-features`, and the vendored crate through its
  manifest.
- `vendored-tests` runs
  `cargo test --manifest-path ncm-api-rs/Cargo.toml --locked --all-targets`.

The equivalent local commands, job by job:

```bash
# build-and-test: default features and --no-default-features, once per toolchain.
# Plain `cargo` already selects the pinned 1.95.0; use `cargo +stable` for the
# stable half of the matrix.
cargo check --workspace --locked --all-targets
cargo test --workspace --locked --all-targets
cargo check --workspace --locked --all-targets --no-default-features
cargo test --workspace --locked --all-targets --no-default-features

# quality (stable only)
cargo +stable fmt --all -- --check
cargo +stable fmt --manifest-path ncm-api-rs/Cargo.toml -- --check
cargo +stable clippy --locked --all-targets --all-features -- -D warnings
cargo +stable clippy --locked --all-targets --no-default-features -- -D warnings
cargo +stable clippy --manifest-path ncm-api-rs/Cargo.toml --locked --all-targets -- -D warnings

# vendored-tests
cargo test --manifest-path ncm-api-rs/Cargo.toml --locked --all-targets
```

A bare `cargo clippy` is not the gate: the `quality` job adds `-D warnings`, covers both feature sets, and lints the vendored crate through its own manifest, so passing it locally does not mean that job will pass.

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
