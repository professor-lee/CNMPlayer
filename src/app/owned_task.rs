use crate::render::wake::WakeSignal;
use compio::runtime::JoinHandle;
use futures::{FutureExt, future::Shared};
use std::pin::Pin;
use std::rc::Rc;

type SharedOutput<T> = Shared<Pin<Box<dyn Future<Output = Option<T>>>>>;

/// The last UI owner cancels the runner. The runner never owns this guard.
/// A completed output can be shared cheaply without detaching pending requests.
#[derive(Clone)]
pub struct SharedTask<T> {
    output: SharedOutput<T>,
    _runner: Option<Rc<JoinHandle<()>>>,
}

impl<T> SharedTask<T> {
    pub fn peek(&self) -> Option<&Option<T>> {
        self.output.peek()
    }

    #[cfg(test)]
    pub(super) fn unstarted(future: Pin<Box<dyn Future<Output = Option<T>>>>) -> Self
    where
        T: Clone,
    {
        Self {
            output: future.shared(),
            _runner: None,
        }
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
mod tests {
    use super::*;
    use futures::channel::oneshot;
    use std::cell::Cell;

    struct Released(Rc<Cell<usize>>);
    impl Drop for Released {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[compio::test]
    async fn last_owner_cancels_pending_work_but_live_clone_keeps_it_alive() {
        let released = Rc::new(Cell::new(0));
        let (started_tx, started_rx) = oneshot::channel();
        let resource = Released(released.clone());
        let task = spawn_shared(
            Box::pin(async move {
                let _resource = resource;
                let _ = started_tx.send(());
                futures::future::pending::<Option<usize>>().await
            }),
            WakeSignal::default(),
        );
        let clone = task.clone();
        started_rx.await.unwrap();
        drop(task);
        compio::time::sleep(std::time::Duration::from_millis(1)).await;
        assert_eq!(released.get(), 0);
        drop(clone);
        compio::time::sleep(std::time::Duration::from_millis(1)).await;
        assert_eq!(released.get(), 1);
    }

    #[compio::test]
    async fn completed_output_remains_available_to_each_owner() {
        let task = spawn_shared(Box::pin(async { Some(42) }), WakeSignal::default());
        let clone = task.clone();
        compio::time::sleep(std::time::Duration::from_millis(1)).await;
        assert_eq!(task.peek(), Some(&Some(42)));
        drop(task);
        assert_eq!(clone.peek(), Some(&Some(42)));
    }
}
