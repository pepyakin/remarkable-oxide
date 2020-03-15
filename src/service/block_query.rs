//! This module gives ability to request block bodies from the given starting block number up to
//! the finalized head.
//!
//! The finalized is specified not as a number but rather a stream which allows incrementally
//! updating the target.

use super::block;
use super::comm::RpcComm;
use super::extendable_range::extendable_range;
use crate::command::Chunk;
use async_std::task;
use futures::prelude::*;
use futures::stream::Stream;
use std::time::Duration;

pub fn stream<'a>(
    start_block_num: u64,
    new_height_finalized_block_num: impl Stream<Item = u64> + Unpin + 'a,
    comm: &'a RpcComm,
) -> impl Stream<Item = Chunk> + 'a {
    extendable_range(start_block_num, new_height_finalized_block_num)
        .then(move |block_num| async move {
            loop {
                match comm.block_hash(block_num).await {
                    Some(block_hash) => break (block_num, block_hash),
                    None => {
                        task::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                }
            }
        })
        .map({
            move |(block_num, block_hash)| async move {
                let block = comm.block_body(block_hash).await;
                let cmds = block::parse_block(block);
                Chunk { cmds, block_num }
            }
        })
        .buffered(3)
}
