//! Windows 平台补丁。
//!
//! 由 `src/main.rs` 以 `#[cfg(windows)] #[path = "../windows/mod.rs"] mod win;` 引入，
//! 只在 Windows 目标上编译；Linux 的构建与运行完全不经过这里。

pub(crate) mod process;
pub(crate) mod stderr;
pub(crate) mod terminal;
