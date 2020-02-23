//! A module that abstracts all concerns regarding connections.
//!
//! This module tries to transparently handle establishing a connection as well as reestablishinging
//! it in case of an error.

use crate::chain_data::{Header, SignedBlock};
use async_std::sync::{Arc, Mutex};
use async_std::task;
use futures::channel::{mpsc, oneshot};
use futures::prelude::*;
use futures::stream::{self, futures_unordered::FuturesUnordered, Stream};
use futures::{future::FutureExt, pin_mut};
use jsonrpsee::{
    client::Subscription,
    core::common::{to_value as to_json_value, Params},
    Client,
};
use log::info;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

struct FinalizedHead {
    subs: Subscription<Header>,
}

impl FinalizedHead {
    async fn subscribe(client: &Client) -> anyhow::Result<Self> {
        let subs: Subscription<Header> = client
            .subscribe(
                "chain_subscribeFinalizedHeads",
                jsonrpsee::core::common::Params::None,
                "chain_unsubscribeFinalizedHeads",
            )
            .await?;
        Ok(Self { subs })
    }

    async fn next(&mut self) -> anyhow::Result<u64> {
        let header = self.subs.next().await;
        let hex_number = header.number.trim_start_matches("0x");
        Ok(u64::from_str_radix(hex_number, 16)?)
    }
}

async fn block_hash(client: &Client, block_number: u64) -> anyhow::Result<Option<String>> {
    let params = Params::Array(vec![to_json_value(block_number)?]);
    let response = client.request("chain_getBlockHash", params).await?;
    Ok(response)
}

async fn block_body(client: &Client, hash: String) -> anyhow::Result<SignedBlock> {
    let params = Params::Array(vec![to_json_value(hash)?]);
    let block = client.request("chain_getBlock", params).await?;
    Ok(block)
}

enum FrontToBack {
    BlockHash {
        id: usize,
        block_num: u64,
        send_back: oneshot::Sender<Option<String>>,
    },
    BlockBody {
        id: usize,
        block_hash: String,
        send_back: oneshot::Sender<SignedBlock>,
    },
}

struct Inner {
    next_id: AtomicUsize,
    finalized_height: Mutex<mpsc::Receiver<u64>>,
    to_back: mpsc::Sender<FrontToBack>,
}

#[derive(Clone)]
pub struct RpcComm {
    inner: Arc<Inner>,
}

impl RpcComm {
    pub fn new(rpc_endpoint: &str) -> Self {
        let (to_back_tx, to_back_rx) = mpsc::channel(16);
        let (finalized_head_tx, finalized_head_rx) = mpsc::channel(16);

        let rpc_endpoint = rpc_endpoint.to_string();
        let _ = task::spawn(
            async move { background_task(rpc_endpoint, to_back_rx, finalized_head_tx) },
        );

        Self {
            inner: Arc::new(Inner {
                next_id: AtomicUsize::new(0),
                to_back: to_back_tx,
                finalized_height: Mutex::new(finalized_head_rx),
            }),
        }
    }

    fn next_id(&self) -> usize {
        self.inner.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub async fn block_hash(&self, block_num: u64) -> Option<String> {
        let id = self.next_id();
        let (send_back_tx, send_back_rx) = oneshot::channel();
        self.inner
            .to_back
            .clone()
            .send(FrontToBack::BlockHash {
                id,
                block_num,
                send_back: send_back_tx,
            })
            .await;
        match send_back_rx.await {
            Ok(hash) => hash,
            Err(_) => todo!(),
        }
    }

    pub async fn block_body(&self, block_hash: String) -> SignedBlock {
        let id = self.next_id();
        let (send_back_tx, send_back_rx) = oneshot::channel();
        self.inner
            .to_back
            .clone()
            .send(FrontToBack::BlockBody {
                id,
                block_hash,
                send_back: send_back_tx,
            })
            .await;
        match send_back_rx.await {
            Ok(block) => block,
            Err(_) => todo!(),
        }
    }

    // TODO: state

    /// Returns the stream that produces the highest finalized block number.
    pub async fn finalized_height(&self) -> u64 {
        // TODO: Unwrap
        self.inner
            .finalized_height
            .lock()
            .await
            .next()
            .await
            .unwrap()
    }
}

async fn background_task(
    rpc_endpoint: String,
    mut from_front: mpsc::Receiver<FrontToBack>,
    finalized_height: mpsc::Sender<u64>,
) {
    // TODO: a container for pending requests that came from the frontend. These requests must be
    // independent from the client since we will drop it from time to time.

    loop {
        match inner_bg_task(&rpc_endpoint, &mut from_front, finalized_height.clone()).await {
            Ok(()) => return,
            Err(err) => {
                info!("connection error: {}. Retrying shortly...", err);
                task::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

async fn inner_bg_task(
    rpc_endpoint: &str,
    from_front: &mut mpsc::Receiver<FrontToBack>,
    mut finalized_height: mpsc::Sender<u64>,
) -> anyhow::Result<()> {
    let client = jsonrpsee::ws_raw_client(rpc_endpoint).await?.into();

    // We will use this stream for all subscriptions.
    let mut unordered_stream = FuturesUnordered::new();

    // TODO: Set the state to connected.
    // TODO: Handle timeout
    // TODO: Handle unwrap
    let mut finalized_head = FinalizedHead::subscribe(&client).await.unwrap();

    // select on futures:
    // - finalized head. Might be moved to directly fire?
    // - one of requests has finished
    // - from_front
    loop {
        let next_event = unordered_stream.next().fuse();
        let next_front = from_front.next().fuse();
        let next_finalized_head = finalized_head.next().fuse();
        pin_mut!(next_event);
        pin_mut!(next_front);
        pin_mut!(next_finalized_head);

        futures::select! {
            e = next_event => {
                // TODO: unwrap
                e.unwrap();
            }
            a = next_finalized_head => {
                // TODO: Reset watchdog
                // TODO: unwrap
                finalized_height.send(a.unwrap()).await.unwrap()
            }
            nf = next_front => {
                match nf {
                    None => return Ok(()),
                    Some(front_to_back) => {
                        handle_next_front_to_back(&client, &mut unordered_stream, front_to_back)
                    }
                }
            }
        }
    }
}

fn handle_next_front_to_back(
    client: &Client,
    unordered_stream: &mut FuturesUnordered<task::JoinHandle<()>>,
    front_to_back: FrontToBack,
) {
    // next front to back command
    match front_to_back {
        FrontToBack::BlockHash {
            id: _,
            block_num,
            send_back,
        } => {
            let client = client.clone();
            unordered_stream.push(task::spawn(async move {
                // TODO: Let's assume that there are no errors.
                let result = block_hash(&client, block_num).await.unwrap();
                let _ = send_back.send(result);
            }));
        }
        FrontToBack::BlockBody {
            id: _,
            block_hash,
            send_back,
        } => {
            let client = client.clone();
            unordered_stream.push(task::spawn(async move {
                // TODO: Let's assume that there are no errors.
                let result = block_body(&client, block_hash).await.unwrap();
                let _ = send_back.send(result);
            }));
        }
    }
}
