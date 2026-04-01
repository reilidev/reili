use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tokio::sync::Notify;

#[derive(Debug, Clone, Default)]
pub struct TaskCancellation {
    cancelled: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl TaskCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub async fn wait_for_cancellation(&self) {
        if self.is_cancelled() {
            return;
        }

        self.notify.notified().await;
    }
}

#[cfg(test)]
mod tests {
    use super::TaskCancellation;

    #[tokio::test]
    async fn cancelled_completes_after_cancel() {
        let cancellation = TaskCancellation::new();
        let waiter = {
            let cancellation = cancellation.clone();
            tokio::spawn(async move {
                cancellation.wait_for_cancellation().await;
            })
        };

        cancellation.cancel();

        waiter.await.expect("join cancellation waiter");
        assert!(cancellation.is_cancelled());
    }
}
