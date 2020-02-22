//! A utility that accepts multiple items but produces only the last one.

use async_std::sync::{Arc, Mutex};
use futures::channel::mpsc;
use futures::prelude::*;

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
        match inner.tx.try_send(()) {
            Ok(()) => {}
            Err(err) if err.is_full() => {}
            Err(err) if err.is_disconnected() => {
                // This means that the rx side is disconnected. For now, just noop, but in future
                // we might consider to propagate the error so the producer has a chance to stop.
            }
            _ => unreachable!(""),
        }
        inner.value = Some(value);
    }
}

pub struct Reader<T> {
    rx: mpsc::Receiver<()>,
    inner: Arc<Mutex<Inner<T>>>,
}

impl<T> Reader<T> {
    pub async fn next(&mut self) -> T {
        // This unwrap cannot panic since self holds on to the arc that contains the tx.
        let _ = self.rx.next().await.unwrap();
        let mut inner = self.inner.lock().await;

        // This unwrap cannot panic since we just received the notification that always implies that
        // the value is set to `Some`.
        inner.value.take().unwrap()
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
