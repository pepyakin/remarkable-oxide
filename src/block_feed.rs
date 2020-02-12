//! Module for accessing the block feed.

use crate::chain_data::{Call, Header, SignedBlock, UncheckedExtrinsic};
use crate::command::{Chunk, Command};
use futures::{future::FutureExt, pin_mut, select};
use jsonrpsee::{
    client::Subscription,
    core::common::{to_value as to_json_value, Params},
    Client,
};

async fn next_finalized(finalized: &mut Subscription<Header>) -> anyhow::Result<u64> {
    let header = finalized.next().await;
    let hex_number = header.number.trim_start_matches("0x");
    Ok(u64::from_str_radix(hex_number, 16)?)
}

async fn block_hash(client: &Client, block_number: u64) -> anyhow::Result<String> {
    let params = Params::Array(vec![to_json_value(block_number)?]);
    let response = client.request("chain_getBlockHash", params).await?;
    Ok(response)
}

async fn block_body(client: &Client, hash: String) -> anyhow::Result<SignedBlock> {
    let params = Params::Array(vec![to_json_value(hash)?]);
    let block = client.request("chain_getBlock", params).await?;
    Ok(block)
}

pub struct ChunkStream {
    client: Client,
    finalized: Subscription<Header>,
    lhs: u64,
    rhs: u64,
}

impl ChunkStream {
    pub async fn new(rpc_endpoint: &str, start_block_num: u64) -> anyhow::Result<Self> {
        let raw_client = jsonrpsee::ws_raw_client(rpc_endpoint).await?;
        let client: Client = raw_client.into();

        let mut finalized: Subscription<Header> = client
            .subscribe(
                "chain_subscribeNewHeads",
                jsonrpsee::core::common::Params::None,
                "chain_unsubscribeNewHeads",
            )
            .await?;

        let rhs = next_finalized(&mut finalized).await?;

        Ok(Self {
            client,
            finalized,
            lhs: start_block_num,
            rhs,
        })
    }

    pub async fn next(&mut self) -> Chunk {
        loop {
            let block_hash = block_hash(&self.client, self.lhs).fuse();
            let next_finalized = next_finalized(&mut self.finalized).fuse();

            pin_mut!(block_hash, next_finalized);

            select! {
                hash = block_hash => {
                    if let Ok(hash) = hash {
                        let body = block_body(&self.client, hash).await.unwrap();
                        let block_num = self.lhs;
                        self.lhs += 1;

                        let mut cmds = vec![];
                        for extrinsic in body.block.extrinsics {
                            use codec::Decode;
                            match <UncheckedExtrinsic<Call>>::decode(&mut &extrinsic.0[..]) {
                                Ok(extrinsic) => {
                                    if let UncheckedExtrinsic::V4 { call: Call::SystemRemark(remark), .. } =  extrinsic {
                                        if let Some(command) = Command::parse(&remark) {
                                            cmds.push(command);
                                        }
                                    }
                                }
                                Err(err) => {
                                    // println!("cannot decode: {:?}", err);
                                }
                            }
                        }

                        return Chunk {
                            block_num,
                            cmds,
                        };
                    }
                },
                finalized = next_finalized => {
                    if let Ok(finalized) = finalized {
                        self.rhs = finalized;
                    }
                }
            }
        }
    }
}
