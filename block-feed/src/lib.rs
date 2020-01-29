//! A program that processes all blocks from the start to the finalized head.
#![recursion_limit = "1024"]

use futures::{future::FutureExt, pin_mut, select};
use jsonrpsee::{
    client::Subscription,
    core::common::{to_value as to_json_value, Params},
    Client,
};

mod chain_data;

use chain_data::{Call, Header, SignedBlock, UncheckedExtrinsic};

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

#[derive(Debug)]
pub struct Command {
    pub x: u16,
    pub y: u16,
    pub rgb: (u8, u8, u8),
}

impl Command {
    fn parse(data: &[u8]) -> Option<Self> {
        use std::fmt::Write;

        const MAGIC: &[u8] = &[0x13, 0x37];
        if !data.starts_with(MAGIC) {
            return None;
        }
        if data.len() != 8 {
            return None;
        }

        let mut coord_buf = String::with_capacity(6);
        for x in &data[2..5] {
            write!(coord_buf, "{:02x}", x).unwrap();
        }
        let x = coord_buf[0..3].parse::<u16>().ok()?;
        let y = coord_buf[3..6].parse::<u16>().ok()?;

        let rgb = (data[5], data[6], data[7]);

        Some(Self { x, y, rgb })
    }
}

pub struct CommandStream {
    client: Client,
    finalized: Subscription<Header>,
    lhs: u64,
    rhs: u64,
}

impl CommandStream {
    pub async fn new() -> anyhow::Result<Self> {
        const RPC: &str = "ws://localhost:1234";
        // const RPC: &str = "wss://kusama-rpc.polkadot.io/";

        let raw_client = jsonrpsee::ws_raw_client(RPC).await.unwrap();
        let client: Client = raw_client.into();

        let mut finalized: Subscription<Header> = client
            .subscribe(
                "chain_subscribeNewHeads",
                jsonrpsee::core::common::Params::None,
                "chain_unsubscribeNewHeads",
            )
            .await
            .unwrap();

        let rhs = next_finalized(&mut finalized).await.unwrap();

        Ok(Self {
            client,
            finalized,
            lhs: 40000,
            rhs,
        })
    }

    pub async fn next(&mut self) -> Vec<Command> {
        loop {
            let block_hash = block_hash(&self.client, self.lhs).fuse();
            let next_finalized = next_finalized(&mut self.finalized).fuse();

            pin_mut!(block_hash, next_finalized);

            select! {
                hash = block_hash => {
                    if let Ok(hash) = hash {
                        let body = block_body(&self.client, hash).await.unwrap();
                        self.lhs += 1;
                        if self.lhs % 1000 == 0 {
                            println!("{}", self.lhs);
                        }

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
                        return cmds;
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

// Run polkadot instance with:
//
// ./polkadot-0.7.16 --wasm-execution Compiled --ws-port 1234
//
fn main() -> anyhow::Result<()> {
    env_logger::init();

    async_std::task::block_on(async move {
        let mut command_stream = CommandStream::new().await.unwrap();
        println!("starting {}..{}", command_stream.lhs, command_stream.rhs);
        loop {
            let command = command_stream.next().await;
            println!("{:?}", command);
        }
    });

    Ok(())
}
