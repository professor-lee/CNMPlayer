use super::{ORDINARY_CAPACITY, PersistenceKey, PersistenceWorker};
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::mpsc::{self, sync_channel};

#[test]
fn flush_waits_for_ordered_jobs_and_reports_failures() {
    let worker = PersistenceWorker::spawn("persistence-test").unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let first = seen.clone();
    worker
        .enqueue(move || {
            first.lock().push(1);
            Ok(())
        })
        .unwrap();
    let second = seen.clone();
    worker
        .enqueue(move || {
            second.lock().push(2);
            Err(anyhow::anyhow!("expected failure"))
        })
        .unwrap();
    assert!(worker.flush().is_err());
    assert_eq!(*seen.lock(), vec![1, 2]);
}

#[test]
fn keyed_updates_keep_latest_value_and_move_to_tail() {
    let worker = PersistenceWorker::spawn("persistence-keyed-test").unwrap();
    let (gate_tx, gate_rx) = sync_channel(0);
    let (started_tx, started_rx) = mpsc::channel();
    let events = Arc::new(Mutex::new(Vec::new()));
    let first_events = events.clone();
    worker
        .enqueue(move || {
            started_tx.send(()).unwrap();
            gate_rx.recv().unwrap();
            first_events.lock().push("ordinary".to_string());
            Ok(())
        })
        .unwrap();
    started_rx.recv().unwrap();

    let config = events.clone();
    worker
        .enqueue_latest(PersistenceKey::Config, move || {
            config.lock().push("config:old".to_string());
            Ok(())
        })
        .unwrap();
    let session = events.clone();
    worker
        .enqueue_latest(PersistenceKey::Session, move || {
            session.lock().push("session".to_string());
            Ok(())
        })
        .unwrap();
    let config = events.clone();
    worker
        .enqueue_latest(PersistenceKey::Config, move || {
            config.lock().push("config:new".to_string());
            Ok(())
        })
        .unwrap();

    gate_tx.send(()).unwrap();
    worker.flush().unwrap();
    assert_eq!(*events.lock(), vec!["ordinary", "session", "config:new"]);
}

#[test]
fn flush_does_not_merge_keyed_updates_across_barrier() {
    let worker = PersistenceWorker::spawn("persistence-barrier-test").unwrap();
    let (gate_tx, gate_rx) = sync_channel(0);
    let (started_tx, started_rx) = mpsc::channel();
    let events = Arc::new(Mutex::new(Vec::new()));
    let first_events = events.clone();
    worker
        .enqueue(move || {
            started_tx.send(()).unwrap();
            gate_rx.recv().unwrap();
            first_events.lock().push("ordinary".to_string());
            Ok(())
        })
        .unwrap();
    started_rx.recv().unwrap();

    let old = events.clone();
    worker
        .enqueue_latest(PersistenceKey::Config, move || {
            old.lock().push("config:before".to_string());
            Ok(())
        })
        .unwrap();

    std::thread::scope(|scope| {
        let flush = scope.spawn(|| worker.flush());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let barrier_queued = loop {
            let queued = {
                let state = worker.shared.state.lock();
                state
                    .queue
                    .iter()
                    .any(|item| matches!(item, super::Item::Flush { .. }))
            };
            if queued {
                break true;
            }
            if std::time::Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        if !barrier_queued {
            gate_tx.send(()).unwrap();
            let _ = flush.join();
            panic!("flush barrier was not admitted");
        }

        let new = events.clone();
        worker
            .enqueue_latest(PersistenceKey::Config, move || {
                new.lock().push("config:after".to_string());
                Ok(())
            })
            .unwrap();
        gate_tx.send(()).unwrap();
        assert!(flush.join().unwrap().is_ok());
    });

    worker.flush().unwrap();
    assert_eq!(
        *events.lock(),
        vec!["ordinary", "config:before", "config:after"]
    );
}

#[test]
fn ordinary_queue_is_bounded_and_rejected_job_is_dropped() {
    let worker = PersistenceWorker::spawn("persistence-capacity-test").unwrap();
    let (gate_tx, gate_rx) = sync_channel(0);
    let (started_tx, started_rx) = mpsc::channel();
    worker
        .enqueue(move || {
            started_tx.send(()).unwrap();
            gate_rx.recv().unwrap();
            Ok(())
        })
        .unwrap();
    started_rx.recv().unwrap();
    for _ in 0..ORDINARY_CAPACITY {
        worker.enqueue(|| Ok(())).unwrap();
    }
    let payload = Arc::new(());
    let payload_weak = Arc::downgrade(&payload);
    let error = worker
        .enqueue(move || {
            drop(payload);
            Ok(())
        })
        .unwrap_err();
    assert!(error.to_string().contains("ordinary queue full"));
    assert!(payload_weak.upgrade().is_none());
    gate_tx.send(()).unwrap();
}

#[test]
fn cloned_handle_rejects_after_shutdown() {
    let worker = PersistenceWorker::spawn("persistence-shutdown-test").unwrap();
    let handle = worker.handle();
    drop(worker);
    assert!(handle.enqueue(|| Ok(())).is_err());
}
