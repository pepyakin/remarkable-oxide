//! A program that processes all blocks from the start to the finalized head.

use futures::{future::FutureExt, pin_mut, prelude::*, select};
use jsonrpsee::{
    client::Subscription,
    core::common::{to_value as to_json_value, Params},
    Client,
};
use serde::Deserialize;

#[derive(Debug, serde::Deserialize)]
struct Header {
    number: String,
}

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

#[derive(Deserialize, Debug)]
struct BlockResponse {
    block: Block,
}

#[derive(Deserialize, Debug)]
struct Block {
    extrinsics: Vec<String>,
}

async fn block_body(client: &Client, hash: String) -> anyhow::Result<BlockResponse> {
    let params = Params::Array(vec![to_json_value(hash)?]);
    let block = client.request("chain_getBlock", params).await?;
    Ok(block)
}

// Run polkadot instance with:
//
// ./polkadot-0.7.16 --wasm-execution Compiled --ws-port 1234
//
fn main() -> anyhow::Result<()> {
    env_logger::init();

    async_std::task::block_on(async move {
        // const RPC: &str = "ws://localhost:1234";
        const RPC: &str = "wss://kusama-rpc.polkadot.io/";

        let mut raw_client = jsonrpsee::ws_raw_client(RPC).await.unwrap();
        let client: Client = raw_client.into();

        let mut finalized: Subscription<Header> = client
            .subscribe(
                "chain_subscribeNewHeads",
                jsonrpsee::core::common::Params::None,
                "chain_unsubscribeNewHeads",
            )
            .await
            .unwrap();

        let mut lhs = 0u64;
        let mut rhs = next_finalized(&mut finalized).await.unwrap();

        println!("starting {}..{}", lhs, rhs);

        loop {
            let block_hash = block_hash(&client, lhs).fuse();
            let next_finalized = next_finalized(&mut finalized).fuse();

            pin_mut!(block_hash, next_finalized);

            select! {
                hash = block_hash => {
                    if let Ok(hash) = hash {
                        println!("{}: {}", lhs, hash);
                        let body = block_body(&client, hash).await;
                        println!("{:#?}", body);
                        lhs += 1;
                    }
                },
                finalized = next_finalized => {
                    if let Ok(finalized) = finalized {
                        rhs = finalized;
                    }
                }
            }
        }
    });

    Ok(())
}
