use crate::render::wake::WakeSignal;
use futures::SinkExt;
use futures::channel::mpsc;
use see::unsync as watch;

/// Exactly one operation runs at a time; newer pending requests replace older ones.
/// Blocking work must finish before returning: cancelling its await is not cancellation.
pub(super) async fn run_latest<Request: Clone, Response>(
    mut requests: watch::Receiver<Option<Request>>,
    mut results: mpsc::Sender<Response>,
    mut process: impl AsyncFnMut(Request) -> Response,
    wake: WakeSignal,
) {
    while requests.changed().await.is_ok() {
        let Some(request) = requests.borrow_and_update().clone() else {
            continue;
        };
        let response = process(request).await;
        if requests.has_changed().unwrap_or(true) {
            continue;
        }
        if results.send(response).await.is_err() {
            break;
        }
        wake.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::oneshot;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[compio::test]
    async fn superseded_requests_never_run_or_publish_stale_results() {
        let (tx, requests) = watch::channel(None);
        let (results, mut rx) = mpsc::channel(1);
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let mut started = Some(started_tx);
        let mut release = Some(release_rx);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let observed = seen.clone();
        let worker = compio::runtime::spawn(run_latest(
            requests,
            results,
            async move |id| {
                observed.borrow_mut().push(id);
                if id == 1 {
                    started.take().unwrap().send(()).unwrap();
                    release.take().unwrap().await.unwrap();
                }
                id
            },
            WakeSignal::default(),
        ));
        tx.send(Some(1)).unwrap();
        started_rx.await.unwrap();
        tx.send(Some(2)).unwrap();
        tx.send(Some(3)).unwrap();
        release_tx.send(()).unwrap();
        use futures::StreamExt;
        assert_eq!(rx.next().await, Some(3));
        assert_eq!(*seen.borrow(), vec![1, 3]);
        drop(tx);
        worker.await.unwrap();
        assert_eq!(rx.next().await, None);
    }

    #[compio::test]
    async fn invalidating_pending_request_discards_active_result() {
        let (tx, requests) = watch::channel(None);
        let (results, mut rx) = mpsc::channel(1);
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let mut started = Some(started_tx);
        let mut release = Some(release_rx);
        let worker = compio::runtime::spawn(run_latest(
            requests,
            results,
            async move |id: u8| {
                started.take().unwrap().send(()).unwrap();
                release.take().unwrap().await.unwrap();
                id
            },
            WakeSignal::default(),
        ));
        tx.send(Some(1)).unwrap();
        started_rx.await.unwrap();
        tx.send(None).unwrap();
        release_tx.send(()).unwrap();
        compio::time::sleep(std::time::Duration::from_millis(1)).await;
        assert!(rx.try_recv().is_err());
        drop(tx);
        worker.await.unwrap();
    }
}
