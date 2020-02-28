//! This module implements a link to a remote WebSocket JSON-RPC node.
//!
//! This link assumes close coupling between it and the remote node. Specifically, that we do not
//! expect any non-transient errors. That is, errors that could be fixed by reconnecting.
//!
//! There are a couple of reasons for that:
//!
//! First, is that at the moment of writing `jsonrpsee`
//! doesn't propagate error so we cannot tell the difference between a, say, JSON-RPC protocol error
//! and a plain connection loss.
//!
//! Second, we don't actually care that much because there is no way to properly recover from these
//! kind of errors without any attention from the user/operator.

use super::{latest, watchdog::Watchdog};
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

/// A link to a remote substrate/polkadot node via WebSocket JSON-RPC connection.
///
/// See the module-level documentation to get more details.
#[derive(Clone)]
pub struct RpcComm {
    inner: Arc<Inner>,
}

impl RpcComm {
    /// Establish a link to the remote endpoint specified by the address.
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

    /// Query the block hash of a block by the given number.
    ///
    /// Returns `None` if the remote node doesn't know about the block with the given number.
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

    /// Query the block body for the given block hash.
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

    // TODO: state
}

async fn background_task(rpc_endpoint: String, mut from_front: mpsc::Receiver<FrontToBack>) {
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

    let mut watchdog = Watchdog::new(Duration::from_secs(10));
    loop {
        let req_finished = inflight_reqs.next().fuse();
        let next_front = from_front.next().fuse();
        let next_finalized_head = finalized_head.next().fuse();
        pin_mut!(req_finished);
        pin_mut!(next_front);
        pin_mut!(next_finalized_head);

        futures::select! {
            () = watchdog.wait().fuse() => anyhow::bail!("watchdog triggered"),
            id = req_finished => {
                if let Some(id) = id {
                    log::trace!("finished request {}", id);
                    // We just fulfilled the request with the given id therefore we need to remove
                    // it from the unfulfilled list ...
                    let _ = unfulfilled_reqs.remove(&id);
                    // ... and reset the watchdog.
                    watchdog.reset();
                }
            }
            new_height = next_finalized_head => {
                // We just received a notification regarding the advancement of the finalized head.
                // That implies that the connection is still alive.
                watchdog.reset();
                // TODO: unwrap
                notify_new_height(height_subscribers, new_height.unwrap()).await
            }
            nf = next_front => {
                match nf {
                    Some(front_to_back) => {
                        handle_next_front_to_back(
                            &client,
                            &mut inflight_reqs,
                            front_to_back,
                            height_subscribers,
                            unfulfilled_reqs
                        )
                    }
                    // Connection to the frontend has been lost meaning that it is shutting down.
                    // Do the same here.
                    None => return Ok(()),
                }
            }
        }
    }
}
