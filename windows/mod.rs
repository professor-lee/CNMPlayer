//! Windows 平台补丁。
//!
//! 由 `src/main.rs` 以 `#[cfg(windows)] #[path = "../windows/mod.rs"] mod win;` 引入，
//! 只在 Windows 目标上编译；Linux 的构建与运行完全不经过这里。
//!
//! 例外：`smtc.rs` 需要访问 `app::mpris_bridge` 的私有类型，由该文件直接以 `#[path]` 挂载。

pub(crate) mod filename;
pub(crate) mod process;
pub(crate) mod stderr;
pub(crate) mod terminal;
