use crate::clock::TickClock;
use crate::config::{Config, MAX_PLAYERS};
use crate::host::Host;
use sendit_sim::Mode;
use std::io;
use std::net::SocketAddr;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Instant;

/// A private loopback deathmatch. Dropping the handle stops its simulation and releases its socket.
pub struct LocalMatch {
    address: SocketAddr,
    shutdown: Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl LocalMatch {
    /// Start a fixed-tick match on the built-in aim arena with one human slot and fixed bots.
    pub fn start(bot_count: u16, skill: f32) -> Result<Self, String> {
        if bot_count >= MAX_PLAYERS {
            return Err(format!(
                "a local match supports up to {} bots",
                MAX_PLAYERS - 1
            ));
        }
        if !(0.0..=1.0).contains(&skill) {
            return Err("bot skill must be between 0 and 1".into());
        }
        let host = Host::bind(Config {
            bind: ([127, 0, 0, 1], 0).into(),
            name: "Local deathmatch".into(),
            mode: Mode::Deathmatch,
            map: "aim".into(),
            capacity: bot_count + 1,
            bots: bot_count,
            bot_skill: skill,
            ..Config::default()
        })?;
        let address = host.address().map_err(|error| error.to_string())?;
        let (shutdown, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("sendit-local-match".into())
            .spawn(move || {
                if let Err(error) = run(host, receiver) {
                    eprintln!("local match stopped: {error}");
                }
            })
            .map_err(|error| format!("start local match: {error}"))?;
        Ok(Self {
            address,
            shutdown,
            worker: Some(worker),
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.address
    }
}

fn run(mut host: Host, shutdown: Receiver<()>) -> io::Result<()> {
    let mut clock = TickClock::new();
    while let Err(RecvTimeoutError::Timeout) =
        shutdown.recv_timeout(clock.delay().unwrap_or_default())
    {
        host.step(Instant::now())?;
        clock.finish(Instant::now());
    }
    host.flush()
}

impl Drop for LocalMatch {
    fn drop(&mut self) {
        let _ = self.shutdown.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sendit_sim::wire::{
        decode, encode, ClientMsg, Command, ServerMsg, FIRST_BOT_ID, MAX_PACKET, PROTOCOL,
    };
    use sendit_sim::Input;
    use std::net::UdpSocket;
    use std::time::Duration;

    fn connect(addr: SocketAddr) -> UdpSocket {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
    }

    fn send(socket: &UdpSocket, packet: ClientMsg) {
        socket.send(&encode(&packet).unwrap()).unwrap();
    }

    fn receive(socket: &UdpSocket) -> ServerMsg {
        let mut buffer = [0u8; MAX_PACKET];
        let size = socket.recv(&mut buffer).unwrap();
        decode(&buffer[..size]).unwrap()
    }

    fn join(socket: &UdpSocket) {
        send(
            socket,
            ClientMsg::Join {
                version: PROTOCOL,
                name: "Local player".into(),
                password: String::new(),
            },
        );
    }

    #[test]
    fn local_bot_match_authenticates_one_player_and_advances_their_input() {
        let game = LocalMatch::start(3, 0.5).unwrap();
        assert!(game.addr().ip().is_loopback());
        let player = connect(game.addr());
        join(&player);
        let ServerMsg::Challenge { token } = receive(&player) else {
            panic!("missing return challenge")
        };
        send(&player, ClientMsg::Confirm { token });
        let ServerMsg::Welcome {
            id,
            map,
            mode,
            token,
            ..
        } = receive(&player)
        else {
            panic!("missing welcome")
        };
        assert_eq!(map, "aim");
        assert_eq!(mode, Mode::Deathmatch);
        send(
            &player,
            ClientMsg::Input {
                token,
                commands: vec![Command {
                    seq: 1,
                    input: Input {
                        strafe: 1.0,
                        ..Input::default()
                    },
                    ..Command::default()
                }],
            },
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(
                Instant::now() < deadline,
                "server never acknowledged movement"
            );
            if let ServerMsg::Snapshot { players, .. } = receive(&player) {
                if players.iter().any(|p| p.id == id && p.ack == 1) {
                    assert_eq!(players.len(), 4);
                    assert_eq!(
                        players
                            .iter()
                            .filter(|p| p.id >= FIRST_BOT_ID && p.ack > 0)
                            .count(),
                        3
                    );
                    break;
                }
            }
        }
        let extra = connect(game.addr());
        join(&extra);
        assert!(matches!(receive(&extra), ServerMsg::Rejected(_)));
    }

    #[test]
    fn stopping_one_match_releases_its_port_and_leaves_the_other_running() {
        let first = LocalMatch::start(0, 0.0).unwrap();
        let second = LocalMatch::start(1, 1.0).unwrap();
        let first_addr = first.addr();
        assert_ne!(first_addr, second.addr());
        drop(first);
        let _released = UdpSocket::bind(first_addr).unwrap();
        let peer = connect(second.addr());
        send(&peer, ClientMsg::Ping { nonce: 42 });
        assert!(matches!(receive(&peer), ServerMsg::Pong { nonce: 42 }));
        assert!(LocalMatch::start(MAX_PLAYERS, 0.5).is_err());
        assert!(LocalMatch::start(1, f32::NAN).is_err());
    }
}
