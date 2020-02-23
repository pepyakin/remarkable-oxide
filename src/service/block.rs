use crate::chain_data::{Call, SignedBlock, UncheckedExtrinsic};
use crate::command::{Chunk, Command};

pub fn parse_block(body: SignedBlock) -> Vec<Command> {
    let mut cmds = vec![];
    for extrinsic in body.block.extrinsics {
        use codec::Decode;
        match <UncheckedExtrinsic<Call>>::decode(&mut &extrinsic.0[..]) {
            Ok(extrinsic) => {
                if let UncheckedExtrinsic::V4 {
                    call: Call::SystemRemark(remark),
                    ..
                } = extrinsic
                {
                    if let Some(command) = Command::parse(&remark) {
                        cmds.push(command);
                    }
                }
            }
            Err(_err) => {
                // println!("cannot decode: {:?}", err);
            }
        }
    }
    cmds
}
