//! Bounded datagrams. No implicit schema negotiation: incompatible clients rejoin
//! with the matching game version.
use crate::{Input, Mode, PlayerView, RoundView, Tuning};
use bincode::Options;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const PROTOCOL: u16 = 1;
pub const MAX_PACKET: usize = 60_000;
/// At most this many recent, unacknowledged commands accompany each client tick.
pub const INPUT_REDUNDANCY: usize = 6;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Command {
    pub seq: u32,
    pub seen_tick: u32,
    pub input: Input,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMsg {
    Join {
        version: u16,
        name: String,
        password: String,
    },
    Input {
        token: u64,
        commands: Vec<Command>,
    },
    Reset {
        token: u64,
        through: u32,
    },
    Leave {
        token: u64,
    },
    Ping {
        nonce: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome {
        id: u16,
        token: u64,
        tick: u32,
        map: String,
        mode: Mode,
        tuning: Tuning,
    },
    Snapshot {
        tick: u32,
        players: Vec<PlayerView>,
        round: RoundView,
    },
    Rejected(String),
    Pong {
        nonce: u64,
    },
    MapData {
        map: String,
        data: Vec<u8>,
    },
}

fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_little_endian()
        .with_varint_encoding()
        .with_limit(MAX_PACKET as u64)
        .reject_trailing_bytes()
}

pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    codec().serialize(value).map_err(|e| e.to_string())
}

pub fn decode<T: DeserializeOwned>(data: &[u8]) -> Result<T, String> {
    if data.len() > MAX_PACKET {
        return Err("oversize datagram".into());
    }
    codec().deserialize(data).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_or_concatenated_packets_are_rejected() {
        let mut bytes = encode(&ClientMsg::Ping { nonce: 42 }).unwrap();
        assert!(matches!(decode(&bytes), Ok(ClientMsg::Ping { nonce: 42 })));
        bytes.push(0);
        assert!(decode::<ClientMsg>(&bytes).is_err());
        assert!(decode::<ClientMsg>(&vec![0; MAX_PACKET + 1]).is_err());
    }
}
