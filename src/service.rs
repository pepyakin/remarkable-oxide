//! High-level logic that orchestrates loading and processing the blocks.

use crate::block_feed;
use crate::command::{Chunk, Command};
use crate::config::Config;
use crate::persist;
use anyhow::Result;
use async_std::task;
use futures::{future::FutureExt, pin_mut, select};
use log::{debug, info, warn, error};
use std::sync::mpsc;
use std::time::Duration;
use std::collections::VecDeque;

pub enum State {
    Connecting,
    Syncing,
    Idle,
}

pub struct Service {
    worker_handle: task::JoinHandle<()>,
    rx: mpsc::Receiver<Chunk>,
    persister: persist::Persister,
    pending: VecDeque<Command>,
}

impl Service {
    pub fn state(&self) -> State {
        todo!()
    }

    pub fn poll(&mut self) -> Option<Command> {
        if let Some(cmd) = self.pending.pop_back() {
            return Some(cmd);
        }
        match self.rx.try_recv() {
            Ok(chunk) => self.pending.extend(chunk.cmds),
            Err(mpsc::TryRecvError::Empty) => {},
            Err(mpsc::TryRecvError::Disconnected) => error!("the worker end hang up unexpectedly"),
        }
        self.pending.pop_back()
    }

    /// Returns the data of the resulting image at the current state.
    ///
    /// This is a blocking call.
    pub fn image_data(&mut self) -> Result<Vec<u8>> {
        task::block_on(async { self.persister.image_data().await })
    }
}

pub fn start(config: Config) -> Result<Service> {
    let (tx, rx) = mpsc::channel();
    let persister = task::block_on(async { persist::start(&config.persisted_data_path).await })?;
    let worker_handle = task::spawn({
        let mut persister = persister.clone();
        async move {
            'toplevel: loop {
                let start_block_num = persister.block_num().await.unwrap();
                let mut stream =
                    match block_feed::ChunkStream::new(&config.rpc_hostname, start_block_num).await
                    {
                        Ok(stream) => stream,
                        Err(err) => {
                            warn!("connection error: {}. Retrying...", err);
                            task::sleep(Duration::from_secs(5)).await;
                            continue;
                        }
                    };
                loop {
                    let chunk = stream.next().fuse();
                    let timeout = task::sleep(Duration::from_secs(5)).fuse();
                    pin_mut!(chunk, timeout);
                    select! {
                        chunk = chunk => {
                            if chunk.block_num % 1000 == 0 {
                                info!("current block: {}", chunk.block_num);
                            }
                            if !chunk.cmds.is_empty() {
                                info!("{} has {} cmds", chunk.block_num, chunk.cmds.len());
                            }
                            if let Err(err) = persister.apply(&chunk).await {
                                warn!("Failed to persist {}", err);
                            }
                            if let Err(_) = tx.send(chunk) {
                                // The other end hung-up. We treat it as a shutdown signal.
                                break 'toplevel;
                            }
                        }
                        timeout = timeout => {
                            warn!("timeout getting chunks. Reconnecting...");
                            continue 'toplevel;
                        }
                    }
                }
            }

            debug!("Shutting down the persister");
            if let Err(err) = persister.shutdown().await {
                warn!(
                    "An error occured while shutting down the persister: {}",
                    err
                );
            }
        }
    });

    Ok(Service {
        worker_handle,
        rx,
        persister,
        pending: VecDeque::new(),
    })
}
