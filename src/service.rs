//! High-level logic that orchestrates loading and processing the blocks.

use crate::command::{Chunk, Command};
use crate::config::Config;
use crate::persist;
use anyhow::Result;
use async_std::task;
use futures::prelude::*;
use futures::pin_mut;
use log::{debug, error, info, warn};
use std::collections::VecDeque;
use std::sync::mpsc;

mod block;
mod comm;
mod hash_query;
mod latest;
mod watchdog;
mod extendable_range;

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
        State::Syncing
    }

    pub fn poll(&mut self) -> Option<Command> {
        if let Some(cmd) = self.pending.pop_back() {
            return Some(cmd);
        }
        match self.rx.try_recv() {
            Ok(chunk) => self.pending.extend(chunk.cmds),
            Err(mpsc::TryRecvError::Empty) => {}
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
    let (persister, start_block_num) = task::block_on(async {
        let mut persister = persist::start(&config.persisted_data_path).await?;
        let start_block_num = persister.block_num().await?;
        Ok::<_, anyhow::Error>((persister, start_block_num))
    })?;
    let worker_handle = task::spawn({
        let mut persister = persister.clone();
        async move {
            let comm = comm::RpcComm::start(&config.rpc_hostname);

            let finalized_height = comm
                .finalized_height()
                .await
                .inspect(|fin_num| info!("finalization advanced to {}", fin_num));
            pin_mut!(finalized_height);
            let stream = hash_query::stream(start_block_num, finalized_height, &comm)
                .map({
                    let comm = &comm;
                    move |(block_num, block_hash)| {
                        // dbg!();
                        async move {
                            let block = comm.block_body(block_hash).await;
                            let cmds = block::parse_block(block);
                            Chunk { cmds, block_num }
                        }
                    }
                })
                .buffered(3);

            pin_mut!(stream);

            loop {
                let chunk = stream.next().await.unwrap();
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
                    // dbg!();
                    return;
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
