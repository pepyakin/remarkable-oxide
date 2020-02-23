//! A utility that accepts multiple items but produces only the last one.

use async_std::sync::Arc;
use futures::channel::mpsc;
use futures::lock::Mutex;
use futures::prelude::*;

use async_std::task;
use std::thread;

pub struct Inner<T> {
    tx: mpsc::Sender<()>,
    value: Option<T>,
}

pub struct Writer<T> {
    inner: Arc<Mutex<Inner<T>>>,
}

impl<T> Writer<T> {
    pub async fn write(&self, value: T) {
        let mut inner = self.inner.lock().await;
        // dbg!();
        inner.value = Some(value);
        match inner.tx.try_send(()) {
            Ok(()) => {
                // dbg!();
            }
            Err(err) if err.is_full() => {
                // dbg!();
            }
            Err(err) if err.is_disconnected() => {
                // dbg!();
                // This means that the rx side is disconnected. For now, just noop, but in future
                // we might consider to propagate the error so the producer has a chance to stop.
            }
            _ => unreachable!(""),
        }
    }
}

pub struct Reader<T> {
    rx: mpsc::Receiver<()>,
    inner: Arc<Mutex<Inner<T>>>,
}

impl<T> Reader<T> {
    pub async fn next(&mut self) -> T {
        loop {
            // dbg!();
            // This unwrap cannot panic since self holds on to the arc that contains the tx.
            let v = self.rx.next().fuse().await.unwrap();
            // dbg!(v);

            let mut inner = self.inner.lock().await;

            if let Some(value) = inner.value.take() {
                return value;
            }
        }
    }
}

pub fn latest<T>() -> (Writer<T>, Reader<T>) {
    let (tx, rx) = mpsc::channel(1);
    let inner = Arc::new(Mutex::new(Inner { tx, value: None }));
    let writer = Writer {
        inner: Arc::clone(&inner),
    };
    let reader = Reader { rx, inner };
    (writer, reader)
}

#[cfg(test)]
mod tests {
    use super::latest;
    use anyhow::Result;

    #[async_std::test]
    async fn sync() {
        let (w, mut r) = latest();

        w.write(1u32).await;
        let v = r.next().await;
        assert_eq!(v, 1);
    }

    #[async_std::test]
    async fn receives_latest() {
        let (w, mut r) = latest();

        w.write(1u32).await;
        w.write(2u32).await;

        let v = r.next().await;
        assert_eq!(v, 2);
    }

    #[async_std::test]
    async fn stress_test() {
        use std::time::Duration;
        let (w, mut r) = latest();

        let t1 = async_std::task::spawn(async move {
            for i in 0u32..10000u32 {
                w.write(i).await;
                if i % 15 == 0 {
                    async_std::task::sleep(Duration::from_millis(10)).await;
                }
            }
        });
        let t2 = async_std::task::spawn(async move {
            loop {
                let p = r.next().await;
            }
        });

        t1.await;
    }
}
