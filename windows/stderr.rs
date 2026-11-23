//! Windows 版 stderr 重定向：与 Unix 的 `dup2(fd, 2)` 对应，把进程的标准错误句柄换成日志文件。

use anyhow::Result;
use std::os::windows::io::{FromRawHandle, IntoRawHandle, OwnedHandle};
use std::path::Path;
use windows_sys::Win32::System::Console::{STD_ERROR_HANDLE, SetStdHandle};

/// Rust 的 `std::io::stderr`（含 panic 输出）每次写入都会重新取 `GetStdHandle`，
/// 换掉之后都落进文件，不再糊在 TUI 画面上。
///
/// 文件以追加方式打开（`FILE_APPEND_DATA`）：`trim_stderr_log_if_needed` 从另一个句柄
/// 把长度归零后，后续写入仍从新的文件末尾开始。
pub(crate) fn redirect_stderr_to_file(path: &Path) -> Result<()> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let raw = file.into_raw_handle();
    // SAFETY: raw 是刚打开的有效文件句柄；成功后它作为标准错误句柄伴随进程到退出，不再关闭。
    if unsafe { SetStdHandle(STD_ERROR_HANDLE, raw) } == 0 {
        let error = std::io::Error::last_os_error();
        // SAFETY: SetStdHandle 失败时没有接管句柄，所有权仍在这里，交给 OwnedHandle 关闭。
        drop(unsafe { OwnedHandle::from_raw_handle(raw) });
        return Err(error.into());
    }
    Ok(())
}
