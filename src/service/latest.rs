//! A utility that accepts multiple items but produces only the last one.

use async_std::sync::Arc;
use futures::channel::mpsc;
use futures::lock::Mutex;
use futures::prelude::*;

struct Inner<T> {
    tx: mpsc::Sender<()>,
    value: Option<T>,
}

struct Setter<T> {
    inner: Arc<Mutex<Inner<T>>>,
}

impl<T> Setter<T> {
    /// Set the latest value notifying the corresponding [`Getter`].
    async fn set(&self, value: T) {
        let mut inner = self.inner.lock().await;

        // Try to send a signal to the getter. Since the `tx` has a capacity of 1 there are two
        // normal cases ...
        match inner.tx.try_send(()) {
            // ... when there were no pending signals ...
            Ok(()) => {}
            // ... or there were. ...
            Err(err) if err.is_full() => {}
            // ... another possible situation is when the other side hung-up.
            //
            // For now, just noop, but in future we might consider to propagate the error so the
            // producer has a chance to stop.
            Err(err) if err.is_disconnected() => {}
            _ => unreachable!(""),
        }

        // In anycase, we update the latest value.
        inner.value = Some(value);
    }
}

struct Getter<T> {
    rx: mpsc::Receiver<()>,
    inner: Arc<Mutex<Inner<T>>>,
}

impl<T> Getter<T> {
    /// A future that returns the last value set by [`Setter::set`].
    async fn get(&mut self) -> T {
        // Receive the notification and then take the lock. Some time can pass between receiving
        // a signal and taking the lock - that's fine.
        let () = self.rx.next().await.unwrap();
        let mut inner = self.inner.lock().await;

        // This unwrap should be fine since the `value` cannot be `None` after we took the lock.
        //
        // The reason for that is that (1) the value is always set by the writer after it releases
        // the lock and because (2) this critical section is only entered after the writer sent a
        // signal to the reader and that can happen only after taking a lock.
        inner.value.take().unwrap()
    }
}

fn latest<T>() -> (Setter<T>, Getter<T>) {
    let (tx, rx) = mpsc::channel(0);
    let inner = Arc::new(Mutex::new(Inner { tx, value: None }));
    let writer = Setter {
        inner: Arc::clone(&inner),
    };
    let reader = Getter { rx, inner };
    (writer, reader)
}

/// Wrap a stream that produces a lot of items and return a stream that produces only the last item
/// that was produced by the original stream.
///
/// Useful to deal with backpressure issues when only the latest value matters.
pub fn wrap_stream<T>(stream: impl Stream<Item=T>) -> impl Stream<Item=T> {
    let (s, g) = latest::<T>();
    stream::unfold(g, |mut g| async move {
        let v = g.get().await;
        Some((v, g))
    })
}

#[cfg(test)]
mod tests {
    use super::latest;
    use anyhow::Result;
    use async_std::task;
    use std::time::Duration;

    #[async_std::test]
    async fn sync() {
        let (s, mut g) = latest();

        s.set(1u32).await;
        let v = g.get().await;
        assert_eq!(v, 1);
    }

    #[async_std::test]
    async fn receives_latest() {
        let (s, mut g) = latest();

        s.set(1u32).await;
        s.set(2u32).await;

        let v = g.next().await;
        assert_eq!(v, 2);
    }

    #[async_std::test]
    async fn stress_test() {
        let (s, mut g) = latest();

        let t1 = task::spawn(async move {
            for i in 0u32..10000u32 {
                s.set(i).await;
                if i % 15 == 0 {
                    task::sleep(Duration::from_millis(10)).await;
                }
            }
        });
        let t2 = task::spawn(async move {
            loop {
                let p = g.get().await;
            }
        });

        t1.await;
        t2.await;
    }
}
