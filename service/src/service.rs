//! High-level logic that orchestrates loading and extracting the commands.
//!
//! Note that the interface here is purely synchronous.

use crate::block_query;
use crate::comm;
use crate::command::{Chunk, Command};
use crate::config::Config;
use crate::latest;
use crate::persist;
use anyhow::Result;
use async_std::task;
use atomic::Atomic;
use futures::pin_mut;
use futures::prelude::*;
use futures::stream;
use log::{debug, error, info, warn};
use std::collections::VecDeque;
use std::sync::{mpsc, Arc};

#[derive(Copy, Clone)]
pub struct StatusReport {
    connection_status: comm::Status,
    current_block: u64,
    finalized_block: u64,
}

pub struct Service {
    _worker_handle: task::JoinHandle<()>,
    rx: mpsc::Receiver<Chunk>,
    persister: persist::Persister,
    pending: VecDeque<Command>,
    // Unfortunately, the struct is bigger than the modern hardware can atomically read or write so
    // we rely on the fallback mode.
    status_report: Arc<Atomic<StatusReport>>,
}

impl Service {
    pub fn status_report(&self) -> StatusReport {
        self.status_report.load(atomic::Ordering::SeqCst)
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
    let status_report = Arc::new(Atomic::new(StatusReport {
        current_block: start_block_num,
        finalized_block: start_block_num,
        connection_status: comm::Status::Connecting,
    }));
    let worker_handle = task::spawn(worker(
        config,
        start_block_num,
        persister.clone(),
        tx,
        Arc::clone(&status_report),
    ));
    Ok(Service {
        _worker_handle: worker_handle,
        rx,
        persister,
        pending: VecDeque::new(),
        status_report,
    })
}

async fn worker(
    config: Config,
    start_block_num: u64,
    mut persister: persist::Persister,
    tx: mpsc::Sender<Chunk>,
    remote_status_report: Arc<Atomic<StatusReport>>,
) {
    enum Event {
        CommStatusChanged(comm::Status),
        NewFinalizedHeight(u64),
        BlockProcessed(Chunk),
    }

    let mut local_status_report = remote_status_report.load(atomic::Ordering::SeqCst);
    let comm = comm::RpcComm::start(&config.rpc_hostname);

    // Obtian the stream that produces the finalized head and then limit it to the latest value.
    // Otherwise, because `block_query::stream` doesn't consume the items for a lot of time there is
    // chance of blowing up the memory consumption.
    let finalized_height = latest::wrap_stream(comm.finalized_height().await);
    pin_mut!(finalized_height);
    let block_ev =
        block_query::stream(start_block_num, finalized_height, &comm).map(Event::BlockProcessed);

    // Then, obtain the second finalized height stream.
    let finalized_height_ev = comm.finalized_height().await.map(Event::NewFinalizedHeight);

    let status_ev = comm.status().await.map(Event::CommStatusChanged);

    let stream = stream::select(block_ev, stream::select(finalized_height_ev, status_ev));
    pin_mut!(stream);
    loop {
        let ev = stream.next().await.unwrap();
        match ev {
            Event::CommStatusChanged(status) => {
                local_status_report.connection_status = status;
                remote_status_report.store(local_status_report, atomic::Ordering::SeqCst);
            }
            Event::NewFinalizedHeight(new_height) => {
                local_status_report.finalized_block = new_height;
                remote_status_report.store(local_status_report, atomic::Ordering::SeqCst);
            }
            Event::BlockProcessed(chunk) => {
                local_status_report.current_block = chunk.block_num;
                remote_status_report.store(local_status_report, atomic::Ordering::SeqCst);

                if chunk.block_num % 1000 == 0 {
                    info!("current block: {}", chunk.block_num);
                }
                if !chunk.cmds.is_empty() {
                    info!("{} has {} cmds", chunk.block_num, chunk.cmds.len());
                }
                if let Err(err) = persister.apply(&chunk).await {
                    // Hopefully the change was atomic. Otherwise, the file might end up corrupted.
                    // TODO: Make it actually atomic.
                    warn!("Failed to persist {}", err);
                }
                if let Err(_) = tx.send(chunk) {
                    // TODO: Handling such an error condition only on attempt to send is not ideal.
                    // Because there might pass quite some time between closing the other part.

                    // The other end hung-up. We treat it as a shutdown signal.
                    break;
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
