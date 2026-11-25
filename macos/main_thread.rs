//! macOS：主线程跑 Cocoa 事件循环，程序主体放到子线程。
//!
//! 「正在播放」的遥控命令（控制中心、媒体键、耳机按键）由 MPRemoteCommandCenter 在主线程的
//! 事件循环里派发，更新 MPNowPlayingInfoCenter 也放在主线程做。终端程序原本把主线程整个交给
//! compio 运行时，没有事件循环，所以 macOS 版的 `main` 把原来的程序主体交给子线程，主线程只初始化
//! NSApplication（Accessory：不在程序坞显示图标、不抢终端焦点）并转事件循环，直到子线程结束。
//! 与 mpv、cliamp 等终端播放器在 macOS 上的做法相同。
//!
//! 其他线程要在主线程做事时，用 `DispatchQueue::main().exec_async`，事件循环会执行这些任务。

use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPort, NSRunLoop};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// 没有任务时最多等这么久再检查一次退出标记（兜底，正常由退出任务唤醒）。
const IDLE_WAKE_SECS: f64 = 1.0;
/// 程序主体原本跑在主线程上（macOS 主线程栈 8 MiB），子线程沿用同样大小，
/// 不改成 Rust 子线程默认的 2 MiB。
const APP_STACK_BYTES: usize = 8 * 1024 * 1024;

/// 在子线程运行 `body`，当前（主）线程转事件循环；`body` 返回后结束循环并交回它的结果。
pub(crate) fn run<F>(body: F) -> anyhow::Result<()>
where
    F: FnOnce() -> anyhow::Result<()> + Send + 'static,
{
    let Some(mtm) = MainThreadMarker::new() else {
        // 只会从 `main` 调用；万一不在主线程就直接运行，「正在播放」不可用但程序照常工作。
        return body();
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let finished = Arc::new(AtomicBool::new(false));
    let exit_signal = ExitSignal(finished.clone());
    let worker = std::thread::Builder::new()
        .name("cnmplayer-app".into())
        .stack_size(APP_STACK_BYTES)
        .spawn(move || {
            // 正常返回与 panic 展开都会经过 drop，主线程不会一直等下去。
            let _exit_signal = exit_signal;
            body()
        })?;

    let run_loop = NSRunLoop::currentRunLoop();
    // 事件循环里没有任何输入源时 runMode:beforeDate: 会立即返回，挂一个空端口让它真正阻塞等待。
    let keep_alive = NSPort::port();
    // SAFETY: 端口与模式都是有效对象；端口在循环结束前一直存活。
    unsafe { run_loop.addPort_forMode(&keep_alive, NSDefaultRunLoopMode) };
    while !finished.load(Ordering::Acquire) {
        // 没有 NSApplication.run 替我们在每轮事件后清空自动释放池，自己包一层；
        // 主队列任务（「正在播放」的更新）也在这一轮里执行。
        autoreleasepool(|_| {
            let limit = NSDate::dateWithTimeIntervalSinceNow(IDLE_WAKE_SECS);
            // SAFETY: 在主线程上以默认模式转一次当前线程的事件循环。
            unsafe { run_loop.runMode_beforeDate(NSDefaultRunLoopMode, &limit) };
        });
    }

    match worker.join() {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// 子线程结束时投递到主队列的退出任务。退出标记只在主线程上置位，主队列先进先出，
/// 子线程退出前投递的其他任务（如清空「正在播放」）一定先执行完。
struct ExitSignal(Arc<AtomicBool>);

impl Drop for ExitSignal {
    fn drop(&mut self) {
        let finished = self.0.clone();
        DispatchQueue::main().exec_async(move || finished.store(true, Ordering::Release));
    }
}
