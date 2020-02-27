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
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

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

#[derive(Debug)]
enum Request {
    BlockHash {
        id: usize,
        block_num: u64,
        send_back: mpsc::Sender<Option<String>>,
    },
    BlockBody {
        id: usize,
        block_hash: String,
        send_back: mpsc::Sender<SignedBlock>,
    },
}

impl Request {
    fn id(&self) -> usize {
        match *self {
            Self::BlockHash { id, .. } | Self::BlockBody { id, .. } => id,
        }
    }

    fn as_future(&self, client: &Client) -> Pin<Box<dyn Future<Output = usize> + Send>> {
        let client = client.clone();
        match self {
            Self::BlockHash {
                id,
                block_num,
                send_back,
            } => {
                Box::pin({
                    let id = *id;
                    let block_num = *block_num;
                    let mut send_back = send_back.clone();
                    async move {
                        // TODO: Let's assume that there are no errors.
                        let result = block_hash(&client, block_num).await.unwrap();
                        let _ = send_back.send(result).await;
                        send_back.close_channel();
                        id
                    }
                })
            }
            Self::BlockBody {
                id,
                block_hash,
                send_back,
            } => {
                Box::pin({
                    let id = *id;
                    let block_hash = block_hash.clone();
                    let mut send_back = send_back.clone();
                    async move {
                        // TODO: Let's assume that there are no errors.
                        let result = block_body(&client, block_hash).await.unwrap();
                        let _ = send_back.send(result).await;
                        send_back.close_channel();
                        id
                    }
                })
            }
        }
    }
}

enum FrontToBack {
    Request(Request),
    SubscribeFinalizedHead { tx: latest::Writer<u64> },
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
    pub fn start(rpc_endpoint: &str) -> Self {
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
        let (send_back_tx, mut send_back_rx) = mpsc::channel(1);
        self.inner
            .to_back
            .clone()
            .send(FrontToBack::Request(Request::BlockHash {
                id,
                block_num,
                send_back: send_back_tx,
            }))
            .await
            .unwrap();
        match send_back_rx.next().await {
            Some(hash) => hash,
            None => todo!(),
        }
    }

    pub async fn block_body(&self, block_hash: String) -> SignedBlock {
        let id = self.next_id();
        let (send_back_tx, mut send_back_rx) = mpsc::channel(1);
        self.inner
            .to_back
            .clone()
            .send(FrontToBack::Request(Request::BlockBody {
                id,
                block_hash,
                send_back: send_back_tx,
            }))
            .await
            .unwrap();
        match send_back_rx.next().await {
            Some(block) => block,
            None => todo!(),
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

    let mut unfulfilled_reqs = HashMap::new();
    let mut height_subscribers = Vec::new();

    loop {
        match inner_bg_task(
            &rpc_endpoint,
            &mut from_front,
            &mut height_subscribers,
            &mut unfulfilled_reqs,
        )
        .await
        {
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

fn handle_next_front_to_back(
    client: &Client,
    inflight_reqs: &mut FuturesUnordered<Pin<Box<dyn Future<Output = usize> + Send>>>,
    front_to_back: FrontToBack,
    height_subscribers: &mut Vec<latest::Writer<u64>>,
    unfulfilled_reqs: &mut HashMap<usize, Request>,
) {
    match front_to_back {
        FrontToBack::Request(req) => {
            log::debug!("a new request received: {:?}", req);
            inflight_reqs.push(req.as_future(client));
            unfulfilled_reqs.insert(req.id(), req);
        }
        FrontToBack::SubscribeFinalizedHead { tx } => {
            height_subscribers.push(tx);
        }
    }
}

async fn inner_bg_task(
    rpc_endpoint: &str,
    from_front: &mut mpsc::Receiver<FrontToBack>,
    height_subscribers: &mut Vec<latest::Writer<u64>>,
    unfulfilled_reqs: &mut HashMap<usize, Request>,
) -> anyhow::Result<()> {
    let client = jsonrpsee::ws_raw_client(rpc_endpoint).await?.into();

    // We will use this stream for all subscriptions.
    let mut inflight_reqs = FuturesUnordered::new();

    // An empty `inflight_reqs` will return `None` all the time. Apparently, this will actually
    // lead to starvation of other futures.
    //
    // To prevent this we add a future that never resolves.
    inflight_reqs.push({
        let fut: Pin<Box<dyn Future<Output = usize> + Send>> = Box::pin(futures::future::pending());
        fut
    });

    // Restart all pending requests that are left from the previous run if any.
    for pending_req in unfulfilled_reqs.values() {
        inflight_reqs.push(pending_req.as_future(&client));
    }

    // TODO: Set the state to connected.
    // TODO: Handle timeout
    // TODO: Handle unwrap
    let mut finalized_head = FinalizedHead::subscribe(&client).await.unwrap();

    // select on futures:
    // - finalized head. Might be moved to directly fire?
    // - one of requests has finished
    // - from_front
    loop {
        let req_finished = inflight_reqs.next().fuse();
        let next_front = from_front.next().fuse();
        let next_finalized_head = finalized_head.next().fuse();
        pin_mut!(req_finished);
        pin_mut!(next_front);
        pin_mut!(next_finalized_head);

        futures::select! {
            id = req_finished => {
                if let Some(id) = id {
                    log::trace!("finished request {}", id);
                    // We just fulfilled the request with the given id therefore we need to remove
                    // it from the unfulfilled list.
                    let _ = unfulfilled_reqs.remove(&id);
                }
            }
            new_height = next_finalized_head => {
                // TODO: Reset watchdog
                // TODO: unwrap
                notify_new_height(height_subscribers, new_height.unwrap()).await
            }
            nf = next_front => {
                match nf {
                    None => return Ok(()),
                    Some(front_to_back) => {
                        handle_next_front_to_back(
                            &client,
                            &mut inflight_reqs,
                            front_to_back,
                            height_subscribers,
                            unfulfilled_reqs
                        )
                    }
                }
            }
        }
    }
}
