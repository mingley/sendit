mod management;

use crate::bots::Bots;
use crate::config::Config;
use crate::discovery::{self, Listing, Publisher};
use crate::stats::Traffic;
use sendit_sim::replay::TraceWriter;
use sendit_sim::wire::{decode, encode, ClientMsg, Command, ServerMsg, CHUNK_BYTES, PROTOCOL};
use sendit_sim::{Arena, Input, Mode, PlayerView, World};
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

const PENDING_LIMIT: usize = 256;
const CHALLENGE_TTL: Duration = Duration::from_secs(3);

struct PendingJoin {
    proof: u64,
    name: String,
    issued: Instant,
}

/// UDP commands carry absolute aim and can arrive after their player has
/// respawned. A snapshot timestamp tells us which life authored the command.
#[derive(Default)]
struct DeathmatchInput {
    life: Option<u32>,
    respawn_tick: Option<u32>,
    reports_ticks: bool,
}

impl DeathmatchInput {
    fn for_tick(
        &mut self,
        view: &PlayerView,
        tick: u32,
        command: Option<Command>,
    ) -> Option<Command> {
        if self
            .life
            .replace(view.life)
            .is_some_and(|old| old != view.life)
        {
            self.respawn_tick = Some(tick);
        }
        let mut command = command?;
        self.reports_ticks |= command.seen_tick != 0;
        if let Some(spawn) = self.respawn_tick {
            let elapsed = tick.wrapping_sub(spawn);
            let saw_spawn = command.seen_tick.wrapping_sub(spawn) <= elapsed;
            // A timestamp-less adapter cannot confirm a snapshot. Publish the
            // new facing for a snapshot interval, then resume legacy commands.
            if saw_spawn || (!self.reports_ticks && elapsed >= SNAPSHOT_INTERVAL) {
                self.respawn_tick = None;
            } else {
                // Consume the sequence so redundant packets cannot reapply it.
                command.input = Input {
                    yaw: view.yaw,
                    pitch: view.pitch,
                    ..Input::default()
                };
            }
        }
        Some(command)
    }
}

struct Session {
    id: u16,
    name: String,
    token: u64,
    last_packet: Instant,
    last_welcome: Option<Instant>,
    last_seq: u32,
    last_reset: Option<u32>,
    pending_welcome: bool,
    deathmatch_input: DeathmatchInput,
    chunk_window: u32,
    chunk_count: u8,
    pending: Vec<Command>,
}

fn new_token() -> io::Result<u64> {
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(u64::from_ne_bytes(random))
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
    bots: Bots,
    socket: UdpSocket,
    sessions: BTreeMap<SocketAddr, Session>,
    challenges: BTreeMap<SocketAddr, PendingJoin>,
    trace: Option<TraceWriter>,
    trace_serial: u32,
    custom_source: Option<Vec<u8>>,
    map_started: Instant,
    publisher: Publisher,
    listing: Listing,
    roster_changed: bool,
    inputs: Vec<(u16, Command)>,
    pub traffic: Traffic,
}

impl Host {
    pub fn bind(config: Config) -> Result<Self, String> {
        let (arena, custom_source) = Arena::load_with_source(&config.map)?;
        for name in &config.rotation {
            if name != &config.map {
                Arena::load(name).map_err(|e| format!("rotation {name}: {e}"))?;
            }
        }
        let mut world = World::new(arena, config.mode, config.tuning);
        world.set_dm_loadout(config.dm_loadout);
        let mut bots = Bots::new(&world.arena, &config);
        bots.fill(&mut world, 0, &config);
        if bots.len() < usize::from(config.bots) {
            return Err("unable to place all fixed bots on this map and team".into());
        }
        let socket =
            UdpSocket::bind(config.bind).map_err(|e| format!("bind {}: {e}", config.bind))?;
        socket.set_nonblocking(true).map_err(|e| e.to_string())?;
        let trace = config
            .record
            .as_ref()
            .map(|path| {
                TraceWriter::create_configured(
                    path,
                    &config.map,
                    "server-session",
                    config.mode,
                    config.tuning,
                )
            })
            .transpose()
            .map_err(|e| format!("open replay: {e}"))?;
        Ok(Self {
            inputs: Vec::with_capacity(usize::from(config.capacity)),
            bots,
            publisher: Publisher::new(config.directory),
            listing: Listing {
                name: config.name.clone(),
                map: config.map.clone(),
                mode: config.mode,
                players: 0,
                cap: config.capacity,
                locked: !config.password.is_empty(),
            },
            trace_serial: 1,
            custom_source,
            map_started: Instant::now(),
            world,
            socket,
            config,
            trace,
            sessions: BTreeMap::new(),
            challenges: BTreeMap::new(),
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

    pub fn bot_count(&self) -> usize {
        self.bots.len()
    }

    fn human_slots_full(&self) -> bool {
        self.sessions.len() >= usize::from(self.config.capacity - self.config.bots)
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
        players.extend(self.bots.names());
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

        if self.sessions.contains_key(&addr) {
            return self.repeat_welcome(addr, now);
        }
        if self.human_slots_full() {
            return self.send(addr, ServerMsg::Rejected("server full".into()));
        }
        let proof = if let Some(pending) = self.challenges.get(&addr) {
            pending.proof
        } else {
            if self.challenges.len() >= PENDING_LIMIT {
                return Ok(());
            }
            let proof = new_token()?;
            self.challenges.insert(
                addr,
                PendingJoin {
                    proof,
                    name: name.chars().take(24).collect(),
                    issued: now,
                },
            );
            proof
        };
        self.send(addr, ServerMsg::Challenge { token: proof })
    }

    fn confirm(&mut self, addr: SocketAddr, proof: u64, now: Instant) -> io::Result<()> {
        if self.sessions.get(&addr).is_some_and(|s| s.token == proof) {
            return self.repeat_welcome(addr, now);
        }
        if !self.challenges.get(&addr).is_some_and(|p| p.proof == proof) {
            return Ok(());
        }
        let pending = self.challenges.remove(&addr).unwrap();
        if self.human_slots_full() {
            return self.send(addr, ServerMsg::Rejected("server full".into()));
        }
        let id = (1..=self.config.capacity).find(|id| self.sessions.values().all(|s| s.id != *id));
        let Some(id) = id else {
            return self.send(addr, ServerMsg::Rejected("server full".into()));
        };
        let desired = Bots::desired(self.sessions.len() + 1, &self.config);
        self.bots.make_room(&mut self.world, desired);
        if !self.world.add(id) {
            self.bots
                .fill(&mut self.world, self.sessions.len(), &self.config);
            return self.send(addr, ServerMsg::Rejected("no free player slot".into()));
        }
        self.roster_changed = true;
        println!("join id={id} name={:?}", pending.name);
        self.sessions.insert(
            addr,
            Session {
                id,
                name: pending.name,
                token: proof,
                last_packet: now,
                last_welcome: None,
                last_seq: 0,
                last_reset: None,
                pending_welcome: true,
                deathmatch_input: DeathmatchInput::default(),
                chunk_window: 0,
                chunk_count: 0,
                pending: Vec::with_capacity(MAX_QUEUED_COMMANDS),
            },
        );
        self.repeat_welcome(addr, now)
    }

    fn repeat_welcome(&mut self, addr: SocketAddr, now: Instant) -> io::Result<()> {
        let session = self.sessions.get_mut(&addr).unwrap();
        if session
            .last_welcome
            .is_some_and(|last| now.duration_since(last) < Duration::from_millis(100))
        {
            return Ok(());
        }
        session.last_welcome = Some(now);
        self.welcome(addr)?;
        self.send(addr, self.snapshot())
    }

    fn welcome(&mut self, addr: SocketAddr) -> io::Result<()> {
        let session = &self.sessions[&addr];
        let (id, token) = (session.id, session.token);
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
        )
    }

    fn map_chunk(
        &mut self,
        addr: SocketAddr,
        token: u64,
        part: u8,
        now: Instant,
    ) -> io::Result<()> {
        let Some(session) = self.sessions.get_mut(&addr).filter(|s| s.token == token) else {
            return Ok(());
        };
        let Some(source) = self.custom_source.as_deref() else {
            return Ok(());
        };
        let Some(chunk) = source.chunks(CHUNK_BYTES).nth(usize::from(part)) else {
            return Ok(());
        };
        let window = self.world.tick / 16;
        if session.chunk_window != window {
            session.chunk_window = window;
            session.chunk_count = 0;
        }
        if session.chunk_count < 16 {
            session.chunk_count += 1;
            session.last_packet = now;
            session.pending_welcome = false;
            let packet = ServerMsg::MapChunk {
                token,
                map: self.config.map.clone(),
                part,
                count: source.len().div_ceil(CHUNK_BYTES) as u8,
                data: chunk.to_vec(),
            };
            self.send(addr, packet)?;
        }
        Ok(())
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
            if len > MAX_CLIENT_PACKET {
                continue;
            }
            if discovery::matches(&buffer[..len]) {
                self.listing.players = (self.sessions.len() + self.bots.len()) as u16;
                self.publisher
                    .receive(&self.socket, addr, &buffer[..len], &self.listing);
                continue;
            }
            self.traffic.received += len as u64;
            let Ok(message) = decode::<ClientMsg>(&buffer[..len]) else {
                continue;
            };
            match message {
                ClientMsg::MapRequest { token, part } => self.map_chunk(addr, token, part, now)?,
                ClientMsg::Confirm { token } => self.confirm(addr, token, now)?,
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
                        session.pending_welcome = false;
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
        self.rotate_if_due(now);
        self.challenges
            .retain(|_, entry| now.duration_since(entry.issued) < CHALLENGE_TTL);
        self.receive(now)?;
        self.inputs.clear();
        self.sessions.retain(|_, session| {
            if now.duration_since(session.last_packet) >= SESSION_TIMEOUT {
                self.world.remove(session.id);
                self.roster_changed = true;
                println!("timeout id={}", session.id);
                return false;
            }
            let mut command = session.next_command();
            if self.config.mode == Mode::Deathmatch {
                if let Some(view) = self.world.view(session.id) {
                    command = session
                        .deathmatch_input
                        .for_tick(view, self.world.tick, command);
                }
            }
            if let Some(command) = command {
                self.inputs.push((session.id, command));
            }
            true
        });
        if self.roster_changed {
            self.bots
                .fill(&mut self.world, self.sessions.len(), &self.config);
        }
        self.inputs.sort_unstable_by_key(|(id, _)| *id);
        if let Some(trace) = &mut self.trace {
            for (id, command) in &self.inputs {
                if let Some(view) = self.world.view(*id) {
                    trace.record_command(self.world.tick, *command, *view)?;
                }
            }
        }
        self.bots.commands(&self.world, &mut self.inputs);
        self.world.advance(&self.inputs);
        if self.config.directory.is_some() && self.world.tick % sendit_sim::TICK_RATE == 1 {
            self.listing.players = (self.sessions.len() + self.bots.len()) as u16;
            self.publisher.heartbeat(&self.socket, now, &self.listing);
        }
        if !self.sessions.is_empty() {
            if self.world.tick % (sendit_sim::TICK_RATE / 2) == 0 {
                let retry: Vec<_> = self
                    .sessions
                    .iter()
                    .filter(|(_, s)| s.pending_welcome)
                    .map(|(addr, _)| *addr)
                    .collect();
                for addr in retry {
                    self.welcome(addr)?;
                }
            }
            if self.world.tick % SNAPSHOT_INTERVAL == 0 {
                self.broadcast(self.snapshot())?;
            }
            if self.roster_changed || self.world.tick % sendit_sim::TICK_RATE == 0 {
                self.broadcast(self.roster())?;
            }
        }
        self.roster_changed = false;
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
    fn deathmatch_inflight_input_cannot_turn_or_fire_before_the_new_life_snapshot() {
        use sendit_sim::Tuning;
        let tuning = Tuning {
            shot_interval_ticks: 1,
            recoil_scale: 0.,
            burst_spread: 0.,
            ..Tuning::default()
        };
        let mut world = World::new(Arena::load("drill").unwrap(), Mode::Deathmatch, tuning);
        assert!(world.add(1) && world.add(2));
        let old_life = world.view(2).unwrap().life;
        let old_look = Input {
            yaw: 4.445593,
            pitch: -0.18,
            ..Input::default()
        };
        let mut fence = DeathmatchInput::default();
        let mut published_tick = 0;
        let mut spawn = None;
        let mut died = false;
        while world.tick < 140 {
            let seq = world.tick + 1;
            let queued = Command {
                seq,
                seen_tick: published_tick,
                input: Input {
                    strafe: if died { 1. } else { 0. },
                    fire: died,
                    ..old_look
                },
            };
            let network = fence
                .for_tick(world.view(2).unwrap(), world.tick, Some(queued))
                .unwrap();
            let attacker = Command {
                seq,
                seen_tick: seq,
                input: Input {
                    fire: seq <= 3,
                    ..Input::default()
                },
            };
            world.advance(&[(1, attacker), (2, network)]);
            let me = world.view(2).unwrap();
            died |= me.hp == 0;
            if me.life != old_life && spawn.is_none() {
                spawn = Some((
                    world.tick,
                    *world
                        .arena
                        .spawns
                        .iter()
                        .find(|s| s.pos == me.body.pos)
                        .unwrap(),
                ));
            }
            if world.tick % SNAPSHOT_INTERVAL == 0 {
                published_tick = world.tick;
                if spawn.is_some() {
                    break;
                }
            }
        }
        let (born, marker) = spawn.unwrap();
        assert_eq!(born, 131);
        assert_eq!(published_tick, 132);
        let me = world.view(2).unwrap();
        assert_eq!(
            (me.yaw, me.pitch, me.body.pos),
            (marker.yaw, 0., marker.pos)
        );
        assert!(me.shot.is_none());
        // Another buffered pre-life packet crosses the wire after publication.
        let late = Command {
            seq: 133,
            seen_tick: 128,
            input: old_look,
        };
        let late = fence.for_tick(me, world.tick, Some(late)).unwrap();
        world.advance(&[(2, late)]);
        assert_eq!(world.view(2).unwrap().yaw, marker.yaw);

        // The first real input carrying the new snapshot is effective immediately.
        let fresh = Command {
            seq: 134,
            seen_tick: published_tick,
            input: Input {
                yaw: marker.yaw + 0.31,
                pitch: 0.15,
                strafe: 1.,
                fire: true,
                ..Input::default()
            },
        };
        let fresh = fence
            .for_tick(world.view(2).unwrap(), world.tick, Some(fresh))
            .unwrap();
        world.advance(&[(2, fresh)]);
        let me = world.view(2).unwrap();
        assert_eq!((me.yaw, me.pitch), (fresh.input.yaw, fresh.input.pitch));
        assert_ne!(me.body.pos, marker.pos);
        assert_eq!(me.shot_tick, world.tick);
    }

    #[test]
    fn timestamp_less_adapter_can_control_replacement_life_after_publication() {
        let mut world = World::new(
            Arena::load("drill").unwrap(),
            Mode::Deathmatch,
            Default::default(),
        );
        assert!(world.add(1));
        let mut view = *world.view(1).unwrap();
        let mut fence = DeathmatchInput::default();
        let command = Command {
            input: Input {
                strafe: 1.,
                ..Input::default()
            },
            ..Command::default()
        };
        assert_eq!(
            fence
                .for_tick(&view, 4, Some(command))
                .unwrap()
                .input
                .strafe,
            1.
        );
        view.life += 1;
        assert_eq!(
            fence
                .for_tick(&view, 11, Some(command))
                .unwrap()
                .input
                .strafe,
            0.
        );
        assert_eq!(
            fence
                .for_tick(&view, 15, Some(command))
                .unwrap()
                .input
                .strafe,
            1.
        );
    }

    #[test]
    fn reorder_and_redundancy_never_replay_consumed_commands() {
        let mut session = Session {
            id: 1,
            name: "test".into(),
            token: 1,
            last_packet: Instant::now(),
            last_welcome: None,
            pending_welcome: false,
            deathmatch_input: DeathmatchInput::default(),
            chunk_window: 0,
            chunk_count: 0,
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

    #[track_caller]
    fn request(host: &mut Host, client: &UdpSocket, packet: ClientMsg) {
        let bytes = encode(&packet).unwrap();
        // Each test sends one request at a time. Count its arrival even when the
        // protocol intentionally gives no reply, such as a wrong-session Leave.
        let expected = host.traffic.received + bytes.len() as u64;
        client.send_to(&bytes, host.address().unwrap()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut clock = crate::clock::TickClock::new();
        loop {
            host.step(Instant::now()).unwrap();
            if host.traffic.received == expected {
                return;
            }
            let now = Instant::now();
            assert!(
                now < deadline,
                "host did not receive {packet:?} before the deadline"
            );
            clock.finish(now);
            if let Some(delay) = clock.delay() {
                std::thread::sleep(delay.min(deadline - now));
            }
        }
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
        let ServerMsg::Challenge { token } = receive(&alice) else {
            panic!("expected proof challenge")
        };
        assert_eq!(host.connected(), 0);
        request(&mut host, &bob, ClientMsg::Confirm { token });
        assert_eq!(host.connected(), 0);
        request(&mut host, &alice, ClientMsg::Confirm { token });
        let ServerMsg::Welcome { token, .. } = receive(&alice) else {
            panic!("expected welcome")
        };
        assert!(matches!(receive(&alice), ServerMsg::Snapshot { .. }));
        let roster = loop {
            let message = receive(&alice);
            if !matches!(message, ServerMsg::Snapshot { .. }) {
                break message;
            }
        };
        let ServerMsg::Roster { players } = roster else {
            panic!("expected roster")
        };
        assert_eq!(players, vec![(1, "test".into())]);
        request(&mut host, &bob, ClientMsg::Leave { token });
        assert_eq!(host.connected(), 1);
        request(&mut host, &bob, join("test-room"));
        assert!(matches!(receive(&bob), ServerMsg::Rejected(_)));
        request(&mut host, &alice, ClientMsg::Leave { token });
        request(&mut host, &bob, join("test-room"));
        let ServerMsg::Challenge { token } = receive(&bob) else {
            panic!("expected Bob challenge")
        };
        request(&mut host, &bob, ClientMsg::Confirm { token });
        assert!(matches!(receive(&bob), ServerMsg::Welcome { .. }));
        assert_eq!(host.connected(), 1);
    }

    #[test]
    fn confirmed_humans_displace_fill_and_releases_restore_it() {
        let args = [
            "--bind",
            "127.0.0.1:0",
            "--max-players",
            "3",
            "--bots",
            "1",
            "--fill",
            "3",
        ];
        let mut host =
            Host::bind(Config::parse(args.into_iter().map(str::to_owned)).unwrap()).unwrap();
        assert_eq!(host.bot_count(), 3);
        let client = socket();
        request(&mut host, &client, join(""));
        let ServerMsg::Challenge { token } = receive(&client) else {
            panic!("missing challenge")
        };
        assert_eq!(host.bot_count(), 3);
        request(&mut host, &client, ClientMsg::Confirm { token });
        assert_eq!(host.connected(), 1);
        assert_eq!(host.bot_count(), 2);
        assert_eq!(host.world.views().len(), 3);

        // Rotation retains the real participant, bot identities and assigned teams.
        let members = |h: &Host| {
            h.world
                .views()
                .into_iter()
                .map(|v| (v.id, v.team))
                .collect::<Vec<_>>()
        };
        let before = members(&host);
        host.command("map relay", Instant::now()).unwrap();
        assert_eq!(members(&host), before);
        let source = client.local_addr().unwrap();
        let new_ticket = host.sessions[&source].token;
        assert_ne!(new_ticket, token);
        request(&mut host, &client, ClientMsg::Leave { token: new_ticket });
        assert_eq!(host.connected(), 0);
        assert_eq!(host.bot_count(), 3);
        assert!(host
            .world
            .views()
            .iter()
            .all(|v| v.id >= sendit_sim::wire::FIRST_BOT_ID));
    }
}
