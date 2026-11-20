//! Bounded datagrams. No implicit schema negotiation: incompatible clients rejoin
//! with the matching game version.
use crate::{Input, Mode, PlayerView, RoundView, Tuning};
use bincode::Options;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const PROTOCOL: u16 = 5;
/// UDP player IDs are below this stable server-assigned bot range.
pub const FIRST_BOT_ID: u16 = 1001;
/// Custom maps travel as individually requested sub-MTU datagrams.
pub const CHUNK_BYTES: usize = 900;
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
    Confirm {
        token: u64,
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
    MapRequest {
        token: u64,
        part: u8,
    },
    Ping {
        nonce: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMsg {
    Challenge {
        token: u64,
    },
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
    Roster {
        players: Vec<(u16, String)>,
    },
    Rejected(String),
    Pong {
        nonce: u64,
    },
    MapChunk {
        token: u64,
        map: String,
        part: u8,
        count: u8,
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

    #[test]
    fn full_arsenal_snapshots_and_redundant_commands_fit_wire_bounds() {
        use crate::{Arena, Hit, Weapon, World};
        let mut world = World::new(
            Arena::load("drill").unwrap(),
            Mode::Deathmatch,
            Tuning::default(),
        );
        for id in 1..=32 {
            assert!(world.add(id));
        }
        let mut players = world.views();
        for p in &mut players {
            let input = Input {
                fire: true,
                ..Input::default()
            };
            let mut shot = p
                .rifle
                .tick(input, &p.body, u32::MAX, u32::MAX, &world.tuning)
                .unwrap();
            shot.hit = Some(u16::MAX);
            p.shot = Some(shot);
            p.id = u16::MAX - p.id;
            p.kills = u16::MAX;
            p.deaths = u16::MAX;
            p.money = u16::MAX;
            p.shot_tick = u32::MAX;
            p.life = u32::MAX;
            p.ack = u32::MAX;
            p.rifle.shots_fired = u32::MAX;
            p.last_hit = Some(Hit {
                tick: u32::MAX,
                source: Some(u16::MAX),
                origin: p.body.eye(),
                damage: 100,
                weapon: Some(Weapon::Sniper),
            });
        }
        let snapshot = |count| {
            encode(&ServerMsg::Snapshot {
                tick: u32::MAX,
                players: players[..count].to_vec(),
                round: world.round_view(),
            })
            .unwrap()
        };
        let max_slots = snapshot(32);
        let ServerMsg::Snapshot {
            players: restored, ..
        } = decode(&max_slots).unwrap()
        else {
            panic!("expected full snapshot")
        };
        assert_eq!(restored.len(), 32);
        assert_eq!(restored[31].last_hit, players[31].last_hit);
        assert!(max_slots.len() < MAX_PACKET);
        let commands = vec![
            Command {
                seq: u32::MAX,
                seen_tick: u32::MAX,
                input: Input {
                    equip: Some(Weapon::Smg),
                    ..Input::default()
                }
            };
            INPUT_REDUNDANCY
        ];
        let outbound = encode(&ClientMsg::Input {
            token: u64::MAX,
            commands,
        })
        .unwrap();
        assert!(
            outbound.len() <= 1200,
            "input must fit an unfragmented datagram"
        );
        eprintln!("arsenal payload bytes: six-command input {}, 10-player snapshot {}, 32-player snapshot {}", outbound.len(), snapshot(10).len(), max_slots.len());
    }
}
