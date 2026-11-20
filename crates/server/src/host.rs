use crate::config::Config;
use crate::stats::Traffic;
use sendit_sim::replay::TraceWriter;
use sendit_sim::wire::{decode, encode, ClientMsg, Command, ServerMsg, PROTOCOL};
use sendit_sim::{Arena, Mode, World};
use std::collections::BTreeMap;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

const MAX_COMMANDS_PER_PACKET: usize = 8;
const MAX_QUEUED_COMMANDS: usize = 32;
const MAX_PACKETS_PER_TICK: usize = 256;
const MAX_CLIENT_PACKET: usize = 4096;
const SNAPSHOT_INTERVAL: u32 = 4;
const SESSION_TIMEOUT: Duration = Duration::from_secs(15);

struct Session {
    id: u16,
    name: String,
    token: u64,
    last_packet: Instant,
    last_seq: u32,
    last_reset: Option<u32>,
    pending: Vec<Command>,
}

fn follows(seq: u32, prev: u32) -> bool {
    (seq.wrapping_sub(prev) as i32) > 0
}

impl Session {
    fn reset_through(&mut self, through: u32) -> bool {
        if self.last_reset.is_some_and(|prev| !follows(through, prev)) {
            return false;
        }
        self.last_reset = Some(through);
        if follows(through, self.last_seq) {
            self.last_seq = through;
            self.pending.retain(|command| follows(command.seq, through));
        }
        true
    }

    fn enqueue(&mut self, command: Command) {
        let ahead = command.seq.wrapping_sub(self.last_seq);
        if ahead == 0 || ahead > i32::MAX as u32 {
            return;
        }
        let input = command.input;
        if ![input.forward, input.strafe, input.yaw, input.pitch]
            .iter()
            .all(|value| value.is_finite())
            || input.forward.abs() > 1.0
            || input.strafe.abs() > 1.0
        {
            return;
        }
        if let Err(index) = self
            .pending
            .binary_search_by_key(&ahead, |queued| queued.seq.wrapping_sub(self.last_seq))
        {
            if index < MAX_QUEUED_COMMANDS {
                if self.pending.len() == MAX_QUEUED_COMMANDS {
                    self.pending.pop();
                }
                self.pending.insert(index, command);
            }
        }
    }

    fn next_command(&mut self) -> Option<Command> {
        if self.pending.is_empty() {
            return None;
        }
        let command = self.pending.remove(0);
        self.last_seq = command.seq;
        Some(command)
    }
}

pub struct Host {
    pub world: World,
    pub config: Config,
    socket: UdpSocket,
    sessions: BTreeMap<SocketAddr, Session>,
    trace: Option<TraceWriter>,
    roster_changed: bool,
    inputs: Vec<(u16, Command)>,
    pub traffic: Traffic,
}

impl Host {
    pub fn bind(config: Config) -> Result<Self, String> {
        let arena = Arena::load(&config.map)?;
        let mut world = World::new(arena, config.mode, config.tuning);
        world.set_dm_loadout(config.dm_loadout);
        let socket =
            UdpSocket::bind(config.bind).map_err(|e| format!("bind {}: {e}", config.bind))?;
        socket.set_nonblocking(true).map_err(|e| e.to_string())?;
        let trace = config
            .record
            .as_ref()
            .map(|path| TraceWriter::create(path, &config.map, "server-session"))
            .transpose()
            .map_err(|e| format!("open replay: {e}"))?;
        Ok(Self {
            inputs: Vec::with_capacity(usize::from(config.capacity)),
            world,
            socket,
            config,
            trace,
            sessions: BTreeMap::new(),
            roster_changed: false,
            traffic: Traffic::default(),
        })
    }

    pub fn address(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    pub fn connected(&self) -> usize {
        self.sessions.len()
    }

    fn send_bytes(&mut self, addr: SocketAddr, data: &[u8]) {
        match self.socket.send_to(data, addr) {
            Ok(size) => self.traffic.sent += size as u64,
            Err(_) => self.traffic.send_drops += 1,
        }
    }

    fn send(&mut self, addr: SocketAddr, message: ServerMsg) -> io::Result<()> {
        let bytes = encode(&message).map_err(io::Error::other)?;
        self.send_bytes(addr, &bytes);
        Ok(())
    }

    fn snapshot(&self) -> ServerMsg {
        ServerMsg::Snapshot {
            tick: self.world.tick,
            players: self.world.views(),
            round: self.world.round_view(),
        }
    }

    fn roster(&self) -> ServerMsg {
        let mut players: Vec<_> = self
            .sessions
            .values()
            .map(|s| (s.id, s.name.clone()))
            .collect();
        players.sort_unstable_by_key(|(id, _)| *id);
        ServerMsg::Roster { players }
    }

    fn broadcast(&mut self, message: ServerMsg) -> io::Result<()> {
        let data = encode(&message).map_err(io::Error::other)?;
        for addr in self.sessions.keys() {
            match self.socket.send_to(&data, addr) {
                Ok(size) => self.traffic.sent += size as u64,
                Err(_) => self.traffic.send_drops += 1,
            }
        }
        Ok(())
    }

    fn join(
        &mut self,
        addr: SocketAddr,
        version: u16,
        name: String,
        password: String,
        now: Instant,
    ) -> io::Result<()> {
        let name = name.trim();
        let rejected = if version != PROTOCOL {
            Some("protocol version mismatch")
        } else if password != self.config.password {
            Some("incorrect password")
        } else if name.is_empty() || name.len() > 96 || name.chars().any(char::is_control) {
            Some("invalid player name")
        } else {
            None
        };
        if let Some(reason) = rejected {
            return self.send(addr, ServerMsg::Rejected(reason.into()));
        }

        let (id, token) = if let Some(session) = self.sessions.get_mut(&addr) {
            session.last_packet = now;
            (session.id, session.token)
        } else {
            let id = (1..=self.config.capacity)
                .find(|id| self.sessions.values().all(|session| session.id != *id));
            let Some(id) = id else {
                return self.send(addr, ServerMsg::Rejected("server full".into()));
            };
            let mut random = [0u8; 8];
            getrandom::fill(&mut random).map_err(|e| io::Error::other(e.to_string()))?;
            let token = u64::from_ne_bytes(random);
            if !self.world.add(id) {
                return self.send(addr, ServerMsg::Rejected("no free player slot".into()));
            }
            self.roster_changed = true;
            self.sessions.insert(
                addr,
                Session {
                    id,
                    name: name.chars().take(24).collect(),
                    token,
                    last_packet: now,
                    last_seq: 0,
                    last_reset: None,
                    pending: Vec::with_capacity(MAX_QUEUED_COMMANDS),
                },
            );
            println!("join id={id} name={name:?}");
            (id, token)
        };
        self.send(
            addr,
            ServerMsg::Welcome {
                id,
                token,
                tick: self.world.tick,
                map: self.config.map.clone(),
                mode: self.config.mode,
                tuning: self.config.tuning,
            },
        )?;
        self.send(addr, self.snapshot())
    }

    fn receive(&mut self, now: Instant) -> io::Result<()> {
        let mut buffer = [0u8; MAX_CLIENT_PACKET + 1];
        for _ in 0..MAX_PACKETS_PER_TICK {
            let (len, addr) = match self.socket.recv_from(&mut buffer) {
                Ok(packet) => packet,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            };
            self.traffic.received += len as u64;
            if len > MAX_CLIENT_PACKET {
                continue;
            }
            let Ok(message) = decode::<ClientMsg>(&buffer[..len]) else {
                continue;
            };
            match message {
                ClientMsg::Ping { nonce } => self.send(addr, ServerMsg::Pong { nonce })?,
                ClientMsg::Join {
                    version,
                    name,
                    password,
                } => self.join(addr, version, name, password, now)?,
                ClientMsg::Input { token, commands } => {
                    if commands.len() > MAX_COMMANDS_PER_PACKET {
                        continue;
                    }
                    if let Some(session) = self.sessions.get_mut(&addr).filter(|s| s.token == token)
                    {
                        session.last_packet = now;
                        for command in commands {
                            session.enqueue(command);
                        }
                    }
                }
                ClientMsg::Reset { token, through } => {
                    if matches!(self.config.mode, Mode::Practice | Mode::Surf) {
                        if let Some(session) =
                            self.sessions.get_mut(&addr).filter(|s| s.token == token)
                        {
                            session.last_packet = now;
                            if session.reset_through(through) {
                                self.world.reset_through(session.id, through);
                            }
                        }
                    }
                }
                ClientMsg::Leave { token } => {
                    if self.sessions.get(&addr).is_some_and(|s| s.token == token) {
                        if let Some(session) = self.sessions.remove(&addr) {
                            self.world.remove(session.id);
                            self.roster_changed = true;
                            println!("leave id={}", session.id);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn step(&mut self, now: Instant) -> io::Result<()> {
        self.receive(now)?;
        self.inputs.clear();
        self.sessions.retain(|_, session| {
            if now.duration_since(session.last_packet) >= SESSION_TIMEOUT {
                self.world.remove(session.id);
                self.roster_changed = true;
                println!("timeout id={}", session.id);
                return false;
            }
            if let Some(command) = session.next_command() {
                self.inputs.push((session.id, command));
            }
            true
        });
        self.inputs.sort_unstable_by_key(|(id, _)| *id);
        if let Some(trace) = &mut self.trace {
            for (id, command) in &self.inputs {
                trace.record(self.world.tick, *id, command.input)?;
            }
        }
        self.world.advance(&self.inputs);
        if !self.sessions.is_empty() {
            if self.world.tick % SNAPSHOT_INTERVAL == 0 {
                self.broadcast(self.snapshot())?;
            }
            if self.roster_changed || self.world.tick % sendit_sim::TICK_RATE == 0 {
                self.broadcast(self.roster())?;
                self.roster_changed = false;
            }
        }
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        if let Some(trace) = &mut self.trace {
            trace.flush()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sendit_sim::Input;

    #[test]
    fn reorder_and_redundancy_never_replay_consumed_commands() {
        let mut session = Session {
            id: 1,
            name: "test".into(),
            token: 1,
            last_packet: Instant::now(),
            last_seq: u32::MAX - 2,
            last_reset: None,
            pending: vec![],
        };
        for seq in [1, u32::MAX, 0, u32::MAX - 1, 0, 1] {
            session.enqueue(Command {
                seq,
                ..Command::default()
            });
        }
        for seq in [u32::MAX - 1, u32::MAX, 0, 1] {
            assert_eq!(session.next_command().unwrap().seq, seq);
        }
        session.enqueue(Command {
            seq: u32::MAX,
            ..Command::default()
        });
        session.enqueue(Command {
            seq: 2,
            input: Input {
                yaw: f32::NAN,
                ..Input::default()
            },
            ..Command::default()
        });
        assert!(session.next_command().is_none());
        for seq in [2, 3, 4] {
            session.enqueue(Command {
                seq,
                ..Command::default()
            });
        }
        assert!(session.reset_through(3));
        session.enqueue(Command {
            seq: 2,
            ..Command::default()
        });
        assert!(!session.reset_through(3));
        assert!(!session.reset_through(1));
        assert_eq!(session.next_command().unwrap().seq, 4);
        assert!(session.next_command().is_none());
    }

    fn socket() -> UdpSocket {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        socket
    }

    fn request(host: &mut Host, client: &UdpSocket, packet: ClientMsg) {
        client
            .send_to(&encode(&packet).unwrap(), host.address().unwrap())
            .unwrap();
        host.step(Instant::now()).unwrap();
    }

    fn receive(client: &UdpSocket) -> ServerMsg {
        let mut buffer = [0u8; 4096];
        let (len, _) = client.recv_from(&mut buffer).unwrap();
        decode(&buffer[..len]).unwrap()
    }

    fn join(password: &str) -> ClientMsg {
        ClientMsg::Join {
            version: PROTOCOL,
            name: "test".into(),
            password: password.into(),
        }
    }

    #[test]
    fn joins_enforce_password_capacity_and_session_ownership() {
        let arguments = ["--bind", "127.0.0.1:0", "--max-players", "1"];
        let mut config = Config::parse(arguments.into_iter().map(str::to_owned)).unwrap();
        config.password = "test-room".into();
        let mut host = Host::bind(config).unwrap();
        let alice = socket();
        let bob = socket();
        request(&mut host, &alice, join("incorrect"));
        assert!(matches!(receive(&alice), ServerMsg::Rejected(_)));
        request(&mut host, &alice, join("test-room"));
        let ServerMsg::Welcome { token, .. } = receive(&alice) else {
            panic!("expected welcome")
        };
        assert!(matches!(receive(&alice), ServerMsg::Snapshot { .. }));
        let ServerMsg::Roster { players } = receive(&alice) else {
            panic!("expected roster")
        };
        assert_eq!(players, vec![(1, "test".into())]);
        request(&mut host, &bob, ClientMsg::Leave { token });
        assert_eq!(host.connected(), 1);
        request(&mut host, &bob, join("test-room"));
        assert!(matches!(receive(&bob), ServerMsg::Rejected(_)));
        request(&mut host, &alice, ClientMsg::Leave { token });
        request(&mut host, &bob, join("test-room"));
        assert!(matches!(receive(&bob), ServerMsg::Welcome { .. }));
        assert_eq!(host.connected(), 1);
    }
}
