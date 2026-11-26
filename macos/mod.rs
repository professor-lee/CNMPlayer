//! macOS 平台补丁。
//!
//! 由 `src/main.rs` 以 `#[cfg(target_os = "macos")] #[path = "../macos/mod.rs"] mod mac;` 引入，
//! 只在 macOS 目标上编译；Linux 的构建与运行完全不经过这里。
//!
//! 例外：`now_playing.rs` 需要访问 `app::mpris_bridge` 的私有类型，由该文件直接以 `#[path]` 挂载。

pub(crate) mod audio_output;
pub(crate) mod keybinds;
pub(crate) mod main_thread;
