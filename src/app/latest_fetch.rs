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
mod tests;
