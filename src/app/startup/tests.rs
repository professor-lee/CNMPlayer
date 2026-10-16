use super::super::startup::step;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;
use std::time::Instant;

#[compio::test]
async fn exhausted_startup_budget_does_not_start_restore() {
    let entered = Cell::new(false);
    let result = step(Instant::now(), async {
        entered.set(true);
    })
    .await;
    assert!(result.is_none());
    assert!(!entered.get());
}

#[compio::test]
async fn restore_uses_remaining_budget_and_drops_pending_work() {
    struct Pending(Rc<Cell<bool>>);
    impl Drop for Pending {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }
    let dropped = Rc::new(Cell::new(false));
    let pending = Pending(dropped.clone());
    let result = step(Instant::now() + Duration::from_millis(20), async move {
        let _pending = pending;
        std::future::pending::<()>().await;
    })
    .await;
    assert!(result.is_none());
    assert!(dropped.get());
}

[Showing lines 386-420 of 421. Use :421 to continue]