//! This module allows requesting hashes of finalized blocks from the given starting number to a
//! intermittently updated finalized number.

use super::comm::RpcComm;
use super::latest;
use async_std::task;
use futures::channel::{mpsc, oneshot};
use futures::prelude::*;
use futures::stream::{self, Stream};
use std::time::Duration;

pub fn stream<'a>(
    start_block_num: u64,
    new_height_finalized_block_num: latest::Reader<u64>,
    comm: &'a RpcComm,
) -> impl Stream<Item = String> + 'a {
    struct State {
        cur: u64,
        finalized: u64,
        new_height_finalized_block_num: latest::Reader<u64>,
    }
    let state = State {
        cur: start_block_num,
        finalized: start_block_num,
        new_height_finalized_block_num,
    };

    stream::unfold(state, {
        |mut state| async move {
            let cur = state.cur;
            if state.cur == state.finalized {
                // All the blocks emitted, next wait for a the new height block.
                state.finalized = state.new_height_finalized_block_num.next().await;
            }
            state.cur += 1;
            Some((cur, state))
        }
    })
    .map(move |block_num| {
        async move {
            loop {
                match comm.block_hash(block_num).await {
                    Some(block_hash) => break block_hash,
                    None => {
                        task::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                }
            }
        }
        .into_stream()
    })
    .flatten()
}
