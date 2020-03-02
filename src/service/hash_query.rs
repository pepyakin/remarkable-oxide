//! This module allows requesting hashes of finalized blocks from the given starting number to a
//! intermittently updated finalized number.

use super::comm::RpcComm;
use super::extendable_range::extendable_range;
use async_std::task;
use futures::prelude::*;
use futures::stream::Stream;
use std::time::Duration;

pub fn stream<'a>(
    start_block_num: u64,
    new_height_finalized_block_num: impl Stream<Item = u64> + Unpin + 'a,
    comm: &'a RpcComm,
) -> impl Stream<Item = (u64, String)> + 'a {
    extendable_range(start_block_num, new_height_finalized_block_num).then(
        move |block_num| async move {
            loop {
                match comm.block_hash(block_num).await {
                    Some(block_hash) => break (block_num, block_hash),
                    None => {
                        task::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                }
            }
        },
    )
}
