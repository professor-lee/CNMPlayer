pub(super) fn peek_shared_future<T>(cover_bytes: &Option<SharedTask<T>>) -> Option<&T> {
    cover_bytes.as_ref()?.peek()?.as_ref()
}

/// 句柄不在 `Option` 里时的取值变体。
pub(super) fn peek_shared<T>(fut: &SharedTask<T>) -> Option<&T> {
    fut.peek()?.as_ref()
}

use crate::render::wake::WakeSignal;
use compio::runtime::JoinHandle;
use futures::FutureExt;
use futures::future::Shared;
use std::pin::Pin;
use std::rc::Rc;

type SharedOutput<T> = Shared<Pin<Box<dyn Future<Output = Option<T>>>>>;

/// The last UI owner cancels the runner. The runner never owns this guard.
/// A completed output can be shared cheaply without detaching pending requests.
#[derive(Clone)]
pub(crate) struct SharedTask<T> {
    output: SharedOutput<T>,
    _runner: Option<Rc<JoinHandle<()>>>,
}

impl<T> SharedTask<T> {
    fn peek(&self) -> Option<&Option<T>> {
        self.output.peek()
    }
}
pub(super) fn spawn_shared<T: Clone + 'static>(
    future: Pin<Box<dyn Future<Output = Option<T>>>>,
    wake: WakeSignal,
) -> SharedTask<T> {
    let output = future.shared();
    let runner_output = output.clone();
    let runner = compio::runtime::spawn(async move {
        let _ = runner_output.await;
        wake.notify();
    });
    SharedTask {
        output,
        _runner: Some(Rc::new(runner)),
    }
}

#[cfg(test)]
mod tests;
