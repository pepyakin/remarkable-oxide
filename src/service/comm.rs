//! A module that abstracts all concerns regarding connections.
//!
//! This module tries to transparently handle establishing a connection as well as reestablishinging
//! it in case of an error.

use super::latest;
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
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

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
    SubscribeFinalizedHead {
        tx: latest::Writer<u64>,
    },
}

struct Inner {
    next_id: AtomicUsize,
    to_back: mpsc::Sender<FrontToBack>,
}

#[derive(Clone)]
pub struct RpcComm {
    inner: Arc<Inner>,
}

impl RpcComm {
    pub fn new(rpc_endpoint: &str) -> Self {
        let (to_back_tx, to_back_rx) = mpsc::channel(16);

        let rpc_endpoint = rpc_endpoint.to_string();
        let _ = task::spawn(async move { background_task(rpc_endpoint, to_back_rx).await });

        Self {
            inner: Arc::new(Inner {
                next_id: AtomicUsize::new(0),
                to_back: to_back_tx,
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
            .await
            .unwrap();
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
            .await
            .unwrap();
        match send_back_rx.await {
            Ok(block) => block,
            Err(_) => todo!(),
        }
    }

    // TODO: state
}

impl RpcComm {
    /// Returns the stream that produces the highest finalized block number.
    pub async fn finalized_height(&self) -> impl Stream<Item = u64> {
        let (tx, rx) = latest::latest::<u64>();
        self.inner
            .to_back
            .clone()
            .send(FrontToBack::SubscribeFinalizedHead { tx })
            .await
            .unwrap();

        stream::unfold(rx, |mut rx| async move {
            let next = rx.next().await;
            Some((next, rx))
        })
    }
}

async fn background_task(rpc_endpoint: String, mut from_front: mpsc::Receiver<FrontToBack>) {
    // TODO: a container for pending requests that came from the frontend. These requests must be
    // independent from the client since we will drop it from time to time.

    let mut height_subscribers = vec![];

    loop {
        // dbg!();
        match inner_bg_task(&rpc_endpoint, &mut from_front, &mut height_subscribers).await {
            Ok(()) => return,
            Err(err) => {
                info!("connection error: {}. Retrying shortly...", err);
                task::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

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

async fn inner_bg_task(
    rpc_endpoint: &str,
    from_front: &mut mpsc::Receiver<FrontToBack>,
    height_subscribers: &mut Vec<latest::Writer<u64>>,
) -> anyhow::Result<()> {
    let client = jsonrpsee::ws_raw_client(rpc_endpoint).await?.into();

    // We will use this stream for all subscriptions.
    let mut unordered_stream = FuturesUnordered::new();

    // An empty `unordered_stream` will return `None` all the time. Apparently, this will actually
    // lead to starvation of other futures.
    //
    // To prevent this we add a future that never resolves.
    unordered_stream.push({
        let fut: Pin<Box<dyn Future<Output = ()> + Send>> = Box::pin(futures::future::pending());
        fut
    });

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
            e = next_event => {}
            new_height = next_finalized_head => {
                // TODO: Reset watchdog
                // TODO: unwrap
                notify_new_height(height_subscribers, new_height.unwrap()).await
            }
            nf = next_front => {
                match nf {
                    None => return Ok(()),
                    Some(front_to_back) => {
                        handle_next_front_to_back(&client, &mut unordered_stream, front_to_back, height_subscribers)
                    }
                }
            }
        }
    }
}

async fn notify_new_height(height_subscribers: &mut [latest::Writer<u64>], new_height: u64) {
    log::debug!(
        "notifying {} subscribers about new finalized {}",
        height_subscribers.len(),
        new_height
    );
    for sub in height_subscribers {
        sub.write(new_height).await;
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

fn handle_next_front_to_back(
    client: &Client,
    unordered_stream: &mut FuturesUnordered<Pin<Box<dyn Future<Output = ()> + Send>>>,
    front_to_back: FrontToBack,
    height_subscribers: &mut Vec<latest::Writer<u64>>,
) {
    // next front to back command
    match front_to_back {
        FrontToBack::BlockHash {
            id: _,
            block_num,
            send_back,
        } => {
            let client = client.clone();
            unordered_stream.push(Box::pin(async move {
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
            unordered_stream.push(Box::pin(async move {
                // TODO: Let's assume that there are no errors.
                let result = block_body(&client, block_hash).await.unwrap();
                let _ = send_back.send(result);
            }));
        }
        FrontToBack::SubscribeFinalizedHead { tx } => {
            // dbg!();
            height_subscribers.push(tx);
        }
    }
}
