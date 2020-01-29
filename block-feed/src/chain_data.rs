//! Declaration of various on-chain data structures.

use serde::Deserialize;
use std::fmt;

#[derive(Debug, serde::Deserialize)]
pub struct Header {
    pub number: String,
    // snip...
}

#[derive(Deserialize, Debug)]
pub struct SignedBlock {
    pub block: Block,
    // snip...
}

#[derive(Deserialize, Debug)]
pub struct Block {
    pub extrinsics: Vec<OpaqueExtrinsic>,
    // snip...
}

/// An extrinsic which is represented as an opaque blob.
#[derive(Debug, codec::Decode)]
pub struct OpaqueExtrinsic(pub Vec<u8>);

impl<'a> serde::Deserialize<'a> for OpaqueExtrinsic {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'a>,
    {
        let r = impl_serde::serialize::deserialize(de)?;
        codec::Decode::decode(&mut &r[..])
            .map_err(|e| serde::de::Error::custom(format!("Decode error: {}", e)))
    }
}

pub type AccountId = [u8; 32];
pub type AccountIndex = u32;

#[derive(Debug)]
pub enum Address {
    Id(AccountId),
    Index(AccountIndex),
}

impl codec::Decode for Address {
    fn decode<I: codec::Input>(input: &mut I) -> Result<Self, codec::Error> {
        use codec::Decode;

        fn need_more_than<T: PartialOrd>(a: T, b: T) -> Result<T, codec::Error> {
            if a < b {
                Ok(b)
            } else {
                Err("Invalid range".into())
            }
        }

        Ok(match input.read_byte()? {
            x @ 0x00..=0xef => Address::Index(AccountIndex::from(x as u32)),
            0xfc => Address::Index(AccountIndex::from(
                need_more_than(0xef, u16::decode(input)?)? as u32,
            )),
            0xfd => Address::Index(AccountIndex::from(need_more_than(
                0xffff,
                u32::decode(input)?,
            )?)),
            0xfe => Address::Index(need_more_than(
                0xffffffffu32.into(),
                Decode::decode(input)?,
            )?),
            0xff => Address::Id(Decode::decode(input)?),
            _ => return Err("Invalid address variant".into()),
        })
    }
}

#[derive(codec::Decode)]
pub enum MultiSignature {
    /// An Ed25519 signature.
    Ed25519([u8; 64]),
    /// An Sr25519 signature.
    Sr25519([u8; 64]),
    /// An ECDSA/SECP256k1 signature.
    Ecdsa([u8; 65]),
}

impl fmt::Debug for MultiSignature {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Self::Ed25519(ref raw) => write!(f, "Ed25519({:?})", &raw[..]),
            Self::Sr25519(ref raw) => write!(f, "Sr25519({:?})", &raw[..]),
            Self::Ecdsa(ref raw) => write!(f, "Ecdsa({:?})", &raw[..]),
        }
    }
}

pub type Period = u64;
pub type Phase = u64;

#[derive(Debug)]
pub enum Era {
    Immortal,
    Mortal(Period, Phase),
}

impl codec::Decode for Era {
    fn decode<I: codec::Input>(input: &mut I) -> Result<Self, codec::Error> {
        use codec::Decode;
        let first = input.read_byte()?;
        if first == 0 {
            Ok(Era::Immortal)
        } else {
            let encoded = first as u64 + ((input.read_byte()? as u64) << 8);
            let period = 2 << (encoded % (1 << 4));
            let quantize_factor = (period >> 12).max(1);
            let phase = (encoded >> 4) * quantize_factor;
            if period >= 4 && phase < period {
                Ok(Era::Mortal(period, phase))
            } else {
                Err("Invalid period and phase".into())
            }
        }
    }
}

#[derive(codec::Decode, Debug)]
pub struct Nonce(#[codec(compact)] u32);

pub type Balance = u128;

#[derive(codec::Decode, Debug)]
pub struct TransactionFee(#[codec(compact)] Balance);

#[derive(Debug)]
pub enum Call {
    SystemRemark(Vec<u8>),
    Other(Vec<u8>),
}

fn consume_remaining<I: codec::Input>(input: &mut I) -> Result<Vec<u8>, codec::Error> {
    if let Some(len) = input.remaining_len()? {
        let mut buf = vec![0; len];
        input.read(&mut buf)?;
        Ok(buf)
    } else {
        let mut buf = Vec::new();
        while let Ok(value) = input.read_byte() {
            buf.push(value);
        }
        Ok(buf)
    }
}

impl codec::Decode for Call {
    fn decode<I: codec::Input>(input: &mut I) -> Result<Self, codec::Error> {
        let module = input.read_byte()?;
        let index = input.read_byte()?;
        if module == 0 && index == 1 {
            Ok(Self::SystemRemark(Vec::decode(input)?))
        } else {
            Ok(Self::Other(consume_remaining(input)?))
        }
    }
}

#[derive(Debug)]
pub enum UncheckedExtrinsic<Call> {
    V4 {
        signature: Option<self::v4::Signature>,
        call: Call,
    },
}

impl<Call: codec::Decode> codec::Decode for UncheckedExtrinsic<Call> {
    fn decode<I: codec::Input>(input: &mut I) -> Result<Self, codec::Error> {
        use codec::Decode;
        let version = input.read_byte()?;

        let is_signed = version & 0b1000_0000 != 0;
        let version = version & 0b0111_1111;

        match version {
            4 => Ok(Self::V4 {
                signature: if is_signed {
                    Some(self::v4::Signature::decode(input)?)
                } else {
                    None
                },
                call: Call::decode(input)?,
            }),
            _ => return Err("unknown version".into()),
        }
    }
}

pub mod v4 {
    use super::{Address, Era, MultiSignature, Nonce, TransactionFee};

    #[derive(codec::Decode, Debug)]
    pub struct Extra {
        era: Era,
        nonce: Nonce,
        fee: TransactionFee,
    }

    #[derive(codec::Decode, Debug)]
    pub struct Signature {
        address: Address,
        signature: MultiSignature,
        extra: Extra,
    }
}
