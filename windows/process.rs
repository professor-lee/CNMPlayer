//! Windows 进程存活探测，供流式临时文件（`.part`）的孤儿回收使用。

use std::os::windows::io::{FromRawHandle, OwnedHandle};
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, STILL_ACTIVE};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// `Some(true)` 存活、`Some(false)` 已退出或不存在、`None` 无法判断（调用方按"不回收"处理）。
///
/// 与 Unix 的 `kill(pid, 0)` 一样有 PID 复用的误差：同号新进程会被当成存活，
/// 只会让孤儿文件晚一些回收，不会误删在用文件。
pub(crate) fn process_alive(pid: u32) -> Option<bool> {
    // SAFETY: OpenProcess 只读参数；失败返回空句柄，成功的句柄立即交给 OwnedHandle 负责关闭。
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return match std::io::Error::last_os_error().raw_os_error() {
            // 系统里没有这个 PID。
            Some(code) if code as u32 == ERROR_INVALID_PARAMETER => Some(false),
            // 进程存在，只是属于更高权限的账户。
            Some(code) if code as u32 == ERROR_ACCESS_DENIED => Some(true),
            _ => None,
        };
    }
    // SAFETY: raw 是刚打开、未被其他所有者持有的有效进程句柄。
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };

    let mut exit_code = 0u32;
    // SAFETY: handle 在本作用域内有效，exit_code 是可写的 u32。
    let ok = unsafe { GetExitCodeProcess(raw, &mut exit_code) };
    drop(handle);
    if ok == 0 {
        return None;
    }
    // 已退出但仍有句柄引用的进程同样拿得到句柄，必须看退出码。
    // 恰好以 259 退出的进程会被误判为存活，同样只是推迟回收。
    Some(exit_code == STILL_ACTIVE as u32)
}
