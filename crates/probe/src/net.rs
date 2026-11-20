use sendit_sim::{
    wire::{decode, encode, ClientMsg, Command, ServerMsg, INPUT_REDUNDANCY, PROTOCOL},
    Arena, Body, Input, Mode, PlayerView, Tuning,
};
use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::{ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

pub const PERIOD: Duration = Duration::from_nanos(1_000_000_000 / 128);

#[derive(Clone, Copy, Debug)]
pub struct Impairment {
    pub latency_ms: u32,
    pub jitter_ms: u32,
    pub loss_pct: f32,
    pub seed: u64,
}

impl Default for Impairment {
    fn default() -> Self {
        Self {
            latency_ms: 0,
            jitter_ms: 0,
            loss_pct: 0.0,
            seed: 31,
        }
    }
}

struct Queue {
    config: Impairment,
    random: u64,
    pending: Vec<(Instant, Vec<u8>)>,
    dropped: u64,
}

impl Queue {
    fn new(config: Impairment, stream: u64) -> Self {
        Self {
            random: config
                .seed
                .wrapping_add(0x9e3779b97f4a7c15u64.wrapping_mul(stream + 1)),
            config,
            pending: Vec::new(),
            dropped: 0,
        }
    }

    fn draw(&mut self) -> f64 {
        self.random = self
            .random
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.random >> 11) as f64 / (1u64 << 53) as f64
    }

    fn push(&mut self, data: Vec<u8>) {
        if self.config.loss_pct > 0.0 && self.draw() * 100.0 < self.config.loss_pct as f64 {
            self.dropped += 1;
            return;
        }
        let jitter = (2.0 * self.draw() - 1.0) * self.config.jitter_ms as f64;
        let delay =
            Duration::from_secs_f64((self.config.latency_ms as f64 + jitter).max(0.0) / 1_000.0);
        let time = Instant::now() + delay;
        let at = self.pending.partition_point(|(when, _)| *when <= time);
        self.pending.insert(at, (time, data));
    }

    fn pop(&mut self) -> Option<Vec<u8>> {
        if self
            .pending
            .first()
            .is_some_and(|(when, _)| *when <= Instant::now())
        {
            Some(self.pending.remove(0).1)
        } else {
            None
        }
    }
}

#[derive(Clone)]
pub struct Frame {
    pub tick: u32,
    pub players: Vec<PlayerView>,
}

impl Frame {
    pub fn player(&self, id: u16) -> Option<&PlayerView> {
        self.players.iter().find(|p| p.id == id)
    }
}

pub struct Peer {
    socket: UdpSocket,
    outgoing: Queue,
    incoming: Queue,
    recent: VecDeque<Command>,
    pending: VecDeque<(Command, Body)>,
    latest: Option<Frame>,
    pub id: Option<u16>,
    pub token: u64,
    pub map: String,
    pub mode: Mode,
    pub tuning: Tuning,
    pub predicted: Option<Body>,
    life: Option<u32>,
    pub corrections: Vec<f32>,
    pub seq: u32,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
}

impl Peer {
    pub fn new(addr: &str, net: Impairment, slot: usize) -> Result<Self, String> {
        let remote = addr
            .to_socket_addrs()
            .map_err(|e| format!("resolve {addr}: {e}"))?
            .next()
            .ok_or("no addresses for server")?;
        let local = if remote.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let socket = UdpSocket::bind(local).map_err(|e| e.to_string())?;
        socket.connect(remote).map_err(|e| e.to_string())?;
        socket.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok(Self {
            socket,
            outgoing: Queue::new(net, slot as u64 * 2),
            incoming: Queue::new(net, slot as u64 * 2 + 1),
            recent: VecDeque::new(),
            pending: VecDeque::new(),
            latest: None,
            id: None,
            token: 0,
            map: String::new(),
            mode: Mode::Practice,
            tuning: Tuning::default(),
            predicted: None,
            life: None,
            corrections: Vec::new(),
            seq: 0,
            tx_bytes: 0,
            rx_bytes: 0,
        })
    }

    pub fn drops(&self) -> u64 {
        self.outgoing.dropped + self.incoming.dropped
    }

    pub fn frame(&self) -> Result<&Frame, String> {
        self.latest
            .as_ref()
            .ok_or_else(|| "server hasn't sent a world snapshot".into())
    }

    pub fn own(&self) -> Result<&PlayerView, String> {
        self.frame()?
            .player(self.id.ok_or("no player slot")?)
            .ok_or_else(|| "local player absent".into())
    }

    pub fn send(&mut self, msg: ClientMsg) -> Result<(), String> {
        self.outgoing.push(encode(&msg)?);
        Ok(())
    }

    pub fn join(&mut self, name: &str, password: &str) -> Result<(), String> {
        self.send(ClientMsg::Join {
            version: PROTOCOL,
            name: name.to_owned(),
            password: password.to_owned(),
        })
    }

    pub fn input(&mut self, input: Input, arena: Option<&Arena>) -> Result<(), String> {
        self.seq += 1;
        let seen_tick = self.latest.as_ref().map_or(0, |frame| frame.tick);
        let cmd = Command {
            seq: self.seq,
            seen_tick,
            input,
        };
        if let (Some(arena), Some(body)) = (arena, self.predicted.as_mut()) {
            sendit_sim::step(body, input, arena, &self.tuning);
            self.pending.push_back((cmd, *body));
            if self.pending.len() > 512 {
                return Err("server has not acknowledged four seconds of movement".into());
            }
        }
        self.recent.push_back(cmd);
        if self.recent.len() > INPUT_REDUNDANCY {
            self.recent.pop_front();
        }
        self.send(ClientMsg::Input {
            token: self.token,
            commands: self.recent.iter().copied().collect(),
        })
    }

    fn snapshot(&mut self, frame: Frame, arena: Option<&Arena>) {
        if self
            .latest
            .as_ref()
            .is_some_and(|last| last.tick >= frame.tick)
        {
            return;
        }
        let own = self.id.and_then(|id| frame.player(id));
        if let Some(own) = own {
            // Match the desktop sender: a received snapshot retires ACKed inputs.
            self.recent.retain(|cmd| cmd.seq > own.ack);
        }
        if let (Some(arena), Some(own)) = (arena, own) {
            let new_life = self.life != Some(own.life);
            if new_life {
                self.pending.clear();
                self.life = Some(own.life);
            }
            while self
                .pending
                .front()
                .is_some_and(|(cmd, _)| cmd.seq <= own.ack)
            {
                if let Some((cmd, then)) = self.pending.pop_front() {
                    if cmd.seq == own.ack && !new_life {
                        self.corrections.push(then.pos.distance(own.body.pos));
                    }
                }
            }
            let mut predicted = own.body;
            for (cmd, future) in &mut self.pending {
                sendit_sim::step(&mut predicted, cmd.input, arena, &self.tuning);
                *future = predicted;
            }
            self.predicted = Some(predicted);
        }
        self.latest = Some(frame);
    }

    pub fn poll(&mut self, arena: Option<&Arena>) -> Result<(), String> {
        while let Some(bytes) = self.outgoing.pop() {
            let n = self.socket.send(&bytes).map_err(|e| e.to_string())?;
            if n != bytes.len() {
                return Err("truncated UDP write".into());
            }
            self.tx_bytes += n as u64;
        }
        let mut buf = [0u8; 65_535];
        loop {
            match self.socket.recv(&mut buf) {
                Ok(n) => {
                    self.rx_bytes += n as u64;
                    self.incoming.push(buf[..n].to_vec());
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => return Err(format!("UDP receive: {e}")),
            }
        }
        while let Some(bytes) = self.incoming.pop() {
            match decode::<ServerMsg>(&bytes)? {
                ServerMsg::Welcome {
                    id,
                    token,
                    map,
                    mode,
                    tuning,
                    ..
                } => {
                    if self.id.is_some_and(|old| old != id) {
                        return Err("slot changed during probe".into());
                    }
                    self.id = Some(id);
                    self.token = token;
                    self.map = map;
                    self.mode = mode;
                    self.tuning = tuning;
                }
                ServerMsg::Snapshot { tick, players, .. } => {
                    self.snapshot(Frame { tick, players }, arena)
                }
                ServerMsg::Rejected(reason) => {
                    return Err(format!("server rejected join or input: {reason}"))
                }
                _ => {}
            }
        }
        Ok(())
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        if self.id.is_some() {
            // End the measured interval before teardown. Send the goodbye directly
            // so our own impairment queue cannot strand a reserved player slot.
            if let Ok(bytes) = encode(&ClientMsg::Leave { token: self.token }) {
                let _ = self.socket.send(&bytes);
            }
        }
    }
}
