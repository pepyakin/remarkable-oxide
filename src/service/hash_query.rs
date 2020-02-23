//! This module allows requesting hashes of finalized blocks from the given starting number to a
//! intermittently updated finalized number.

use super::comm::RpcComm;
use super::latest;
use async_std::task;
use futures::prelude::*;
use futures::stream::{self, Stream};
use std::time::Duration;

pub fn stream<'a>(
    start_block_num: u64,
    new_height_finalized_block_num: impl Stream<Item = u64> + 'a,
    comm: &'a RpcComm,
) -> impl Stream<Item = (u64, String)> + 'a {
    struct State<F> {
        cur: u64,
        finalized: u64,
        new_height_finalized_block_num: F,
    }

    stream::unfold(
        State {
            cur: start_block_num,
            finalized: start_block_num,
            new_height_finalized_block_num: Box::pin(new_height_finalized_block_num),
        },
        {
            |mut state| async move {
                log::debug!("unfold, cur {} fin {}", state.cur, state.finalized);
                let cur = state.cur;
                if state.cur >= state.finalized {
                    log::debug!("waiting for the next finalized");
                    // All the blocks emitted, next wait for a the new height block.
                    state.finalized = state.new_height_finalized_block_num.next().await.unwrap();
                }
                // dbg!();
                state.cur += 1;
                Some((cur, state))
            }
        },
    )
    .then(move |block_num| async move {
        loop {
            match comm.block_hash(block_num).await {
                Some(block_hash) => break (block_num, block_hash),
                None => {
                    dbg!();
                    task::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            }
        }
    })
}
