use crate::{
    audio::{Cue, Sound},
    benchmark::InputClock,
    options::Options,
};
use bevy::{
    input::mouse::MouseMotion,
    prelude::*,
    window::{CursorGrabMode, PrimaryWindow},
};
use sendit_sim::{
    motion::{self, DT},
    replay::{Trace, TraceWriter},
    wire::{self, ClientMsg, Command, ServerMsg},
    Arena, Body, Input, Mode, Phase, PlayerView, Rifle, RoundView, Shot, Tuning,
};
use std::{
    collections::{HashMap, VecDeque},
    io::ErrorKind,
    net::{SocketAddr, ToSocketAddrs, UdpSocket},
    time::{Duration, Instant},
};

#[derive(Resource)]
pub struct Session {
    socket: UdpSocket,
    token: u64,
    pub id: Option<u16>,
    pub map: Arena,
    pub map_revision: u64,
    server_map: String,
    map_ready: bool,
    pub mode: Mode,
    pub tuning: Tuning,
    pub players: Vec<PlayerView>,
    names: HashMap<u16, String>,
    pub scoreboard: bool,
    tab_since: Option<Instant>,
    previous: Vec<PlayerView>,
    previous_tick: u32,
    pub server_tick: u32,
    last_snapshot: Option<Instant>,
    last_packet: Instant,
    last_join: Option<Instant>,
    last_ping: Option<(u64, Instant)>,
    pub rtt_ms: Option<f32>,
    pub round: Option<RoundView>,
    pub status: String,
    rejected: bool,
    pub input: Input,
    tap_fire: bool,
    fire_held: bool,
    pub body: Body,
    pub rifle: Rifle,
    pub local_shot: Option<Shot>,
    pub local_shot_time: Option<Instant>,
    sequence: u32,
    pending: VecDeque<Command>,
    pub accumulator: f32,
    spawn: Option<(Body, f32)>,
    reset_life: Option<(u32, Instant, Instant)>,
    pub captured: bool,
    pub show_help: bool,
    pub show_debug: bool,
    pub spectate: Option<u16>,
    pub replay: Option<Trace>,
    replay_tick: u32,
    writer: Option<TraceWriter>,
    recording_failed: bool,
    pub walk_distance: f32,
    pub damage_at: Option<Instant>,
    pub hit_at: Option<Instant>,
}

impl Session {
    pub fn open(opts: &Options) -> Result<Self, String> {
        let address: SocketAddr = opts
            .address
            .to_socket_addrs()
            .map_err(|e| e.to_string())?
            .next()
            .ok_or("host has no address")?;
        let socket = UdpSocket::bind(if address.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        })
        .map_err(|e| e.to_string())?;
        socket.connect(address).map_err(|e| e.to_string())?;
        socket.set_nonblocking(true).map_err(|e| e.to_string())?;
        let map = Arena::load("drill")?;
        let spawn = map.spawns.first().ok_or("drill has no spawn")?;
        let body = Body::at(spawn.pos);
        let input = Input {
            yaw: spawn.yaw,
            ..default()
        };
        let replay = opts.replay.as_deref().map(Trace::load).transpose()?;
        Ok(Self {
            socket,
            token: 0,
            id: None,
            map,
            map_revision: 1,
            server_map: String::new(),
            map_ready: false,
            mode: Mode::Practice,
            tuning: Tuning::default(),
            players: vec![],
            names: HashMap::new(),
            scoreboard: false,
            tab_since: None,
            previous: vec![],
            previous_tick: 0,
            server_tick: 0,
            last_snapshot: None,
            last_packet: Instant::now(),
            last_join: None,
            last_ping: None,
            rtt_ms: None,
            round: None,
            status: format!("Connecting to {}", opts.address),
            rejected: false,
            input,
            tap_fire: false,
            fire_held: false,
            body,
            rifle: Rifle::default(),
            local_shot: None,
            local_shot_time: None,
            sequence: 0,
            pending: VecDeque::with_capacity(128),
            accumulator: 0.,
            spawn: None,
            reset_life: None,
            captured: false,
            show_help: false,
            show_debug: false,
            spectate: None,
            replay,
            replay_tick: 0,
            writer: None,
            recording_failed: false,
            walk_distance: 0.,
            damage_at: None,
            hit_at: None,
        })
    }

    fn send(&self, message: &ClientMsg) {
        if let Ok(payload) = wire::encode(message) {
            // Inputs repeat in consecutive datagrams. A temporarily full local queue can skip one send.
            let _ = self.socket.send(&payload);
        }
    }

    pub fn me(&self) -> Option<&PlayerView> {
        self.players.iter().find(|v| Some(v.id) == self.id)
    }

    pub fn name(&self, id: u16) -> String {
        self.names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("Player {id}"))
    }

    fn can_move(&self) -> bool {
        self.me().is_some_and(|p| p.hp > 0)
            && !(self.mode == Mode::Competitive
                && self
                    .round
                    .as_ref()
                    .is_some_and(|r| matches!(r.phase, Phase::Buy | Phase::Ended)))
    }

    pub fn fraction(&self) -> f32 {
        self.last_snapshot.map_or(1., |last| {
            let span = self.server_tick.saturating_sub(self.previous_tick).max(1);
            (last.elapsed().as_secs_f32() / (span as f32 * DT)).min(1.)
        })
    }

    fn seen_tick(&self) -> u32 {
        self.previous_tick
            + ((self.server_tick.saturating_sub(self.previous_tick) as f32) * self.fraction())
                .round() as u32
    }

    pub fn remote_pose(&self, player: &PlayerView) -> (Vec3, f32) {
        if let Some(old) = self
            .previous
            .iter()
            .find(|p| p.id == player.id && p.life == player.life)
        {
            let alpha = self.fraction();
            let delta_yaw = (player.yaw - old.yaw + std::f32::consts::PI)
                .rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            (
                old.body.pos.lerp(player.body.pos, alpha),
                old.yaw + delta_yaw * alpha,
            )
        } else {
            (player.body.pos, player.yaw)
        }
    }

    fn next_spectator(&mut self) {
        let Some(me) = self.me() else { return };
        let team = me.team;
        let players: Vec<_> = self
            .players
            .iter()
            .filter(|p| p.hp > 0 && p.team == team && Some(p.id) != self.id)
            .map(|p| p.id)
            .collect();
        self.spectate = if players.is_empty() {
            None
        } else {
            let next = players
                .iter()
                .position(|&id| Some(id) == self.spectate)
                .map_or(0, |n| (n + 1) % players.len());
            Some(players[next])
        };
    }

    fn replay_pending(&mut self) {
        if !self.can_move() || !self.map_ready {
            return;
        }
        let armed = self.me().is_some_and(|p| p.armed);
        for (index, cmd) in self.pending.iter().enumerate() {
            motion::step(&mut self.body, cmd.input, &self.map, &self.tuning);
            if armed {
                let mut combat_input = cmd.input;
                combat_input.fire &= self.mode != Mode::Surf;
                self.rifle.tick(
                    combat_input,
                    &self.body,
                    self.server_tick + index as u32 + 1,
                    sendit_sim::shot_seed(self.id.unwrap_or_default(), cmd.seq),
                    &self.tuning,
                );
            }
        }
    }

    fn snapshot(
        &mut self,
        tick: u32,
        players: Vec<PlayerView>,
        round: RoundView,
        audio: &mut Sound,
    ) {
        if self.id.is_none() || (self.last_snapshot.is_some() && tick <= self.server_tick) {
            return;
        }
        let old_self = self.me().map(|p| (p.life, p.hp, p.kills));
        self.previous_tick = if self.last_snapshot.is_some() {
            self.server_tick
        } else {
            tick
        };
        self.server_tick = tick;
        self.last_snapshot = Some(Instant::now());
        self.previous = std::mem::replace(&mut self.players, players);
        self.round = Some(round);
        for player in &self.players {
            if Some(player.id) == self.id {
                continue;
            }
            if let Some(old) = self
                .previous
                .iter()
                .find(|p| p.id == player.id && p.life == player.life)
            {
                if old.shot_tick != player.shot_tick && player.shot.is_some() {
                    audio.emit(
                        Cue::Shot,
                        player.body.pos + Vec3::Y * motion::EYE_HEIGHT,
                        true,
                    );
                }
                if player.hp > 0 && player.body.grounded {
                    audio.walk(player.id, player.body.pos, old.body.pos);
                }
            }
        }
        let Some(me) = self.me() else { return };
        let (body, rifle, ack, life, hp, kills, yaw) =
            (me.body, me.rifle, me.ack, me.life, me.hp, me.kills, me.yaw);
        if self
            .reset_life
            .is_some_and(|(previous_life, _, _)| previous_life == life)
        {
            return;
        }
        self.reset_life = None;
        let changed_life = old_self.is_none_or(|p| p.0 != life);
        if changed_life {
            self.spawn = Some((body, yaw));
            self.input.yaw = yaw;
            self.input.pitch = 0.;
            self.pending.clear();
            self.accumulator = 0.;
            self.local_shot = None;
            self.walk_distance = 0.;
        } else if let Some((_, old_hp, old_kills)) = old_self {
            if hp < old_hp {
                self.damage_at = Some(Instant::now());
                audio.emit(Cue::Damage, body.pos, false);
            }
            if kills > old_kills {
                self.hit_at = Some(Instant::now());
            }
        }
        if hp == 0
            && !self.players.iter().any(|p| {
                Some(p.id) == self.spectate && p.hp > 0 && p.team == self.me().unwrap().team
            })
        {
            self.next_spectator();
        } else if hp > 0 {
            self.spectate = None;
        }
        self.body = body;
        self.rifle = rifle;
        self.sequence = self.sequence.max(ack);
        while self.pending.front().is_some_and(|c| c.seq <= ack) {
            self.pending.pop_front();
        }
        self.replay_pending();
        if self.map_ready && !self.recording_failed {
            self.status.clear();
        }
    }
}

pub fn receive(
    opts: Res<Options>,
    mut s: ResMut<Session>,
    mut sound: ResMut<Sound>,
    mut exit: EventWriter<AppExit>,
) {
    let now = Instant::now();
    let mut bytes = [0u8; 65536];
    for _ in 0..64 {
        let len = match s.socket.recv(&mut bytes) {
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) => {
                if s.id.is_none() {
                    s.status = format!("Waiting for {} | {e}", opts.address)
                };
                break;
            }
        };
        let Ok(message) = wire::decode::<ServerMsg>(&bytes[..len]) else {
            continue;
        };
        s.last_packet = now;
        match message {
            ServerMsg::Welcome {
                id,
                token,
                tick,
                map,
                mode,
                tuning,
            } => {
                if s.id != Some(id) || s.token != token {
                    s.last_snapshot = None;
                    s.sequence = 0;
                    s.players.clear();
                    s.pending.clear();
                    s.server_tick = tick;
                }
                s.token = token;
                s.id = Some(id);
                s.mode = mode;
                s.tuning = tuning;
                if s.server_map != map || !s.map_ready {
                    s.map_ready = false;
                    if let Ok(arena) = Arena::load(&map) {
                        s.map = arena;
                        s.map_ready = true;
                        s.map_revision += 1;
                    }
                    if s.replay
                        .as_ref()
                        .is_some_and(|trace| trace.header.map != map)
                    {
                        eprintln!("input replay was recorded on a different map");
                        exit.send(AppExit::error());
                    }
                    s.server_map = map;
                }
                s.status = if s.map_ready {
                    "Waiting for match state".into()
                } else {
                    "Waiting for map".into()
                };
            }
            ServerMsg::MapData { map, data } => {
                if map == s.server_map {
                    match Arena::from_json(&data) {
                        Ok(arena) => {
                            s.map = arena;
                            s.map_ready = true;
                            s.map_revision += 1;
                        }
                        Err(e) => s.status = format!("Map rejected | {e}"),
                    }
                }
            }
            ServerMsg::Roster { players } => {
                s.names = players
                    .into_iter()
                    .map(|(id, name)| {
                        let clean: String = name
                            .chars()
                            .take(24)
                            .map(|ch| {
                                if ch.is_ascii_graphic() || ch == ' ' {
                                    ch
                                } else {
                                    '?'
                                }
                            })
                            .collect();
                        (id, clean)
                    })
                    .collect();
            }
            ServerMsg::Snapshot {
                tick,
                players,
                round,
            } => s.snapshot(tick, players, round, &mut sound),
            ServerMsg::Rejected(reason) => {
                s.status = format!("Join rejected | {reason}");
                eprintln!("{}", s.status);
                s.rejected = true;
                if opts.benchmark.is_some() {
                    exit.send(AppExit::error());
                }
            }
            ServerMsg::Pong { nonce } => {
                if let Some((n, sent)) = s.last_ping {
                    if n == nonce {
                        s.rtt_ms = Some(sent.elapsed().as_secs_f32() * 1000.);
                    }
                }
            }
        }
    }
    if s.id.is_some() && s.last_packet.elapsed() > Duration::from_secs(5) {
        s.id = None;
        s.players.clear();
        s.pending.clear();
        s.last_snapshot = None;
        s.status = format!("Reconnecting to {}", opts.address);
    }
    if s.id.is_none()
        && !s.rejected
        && s.last_join
            .is_none_or(|i| i.elapsed() > Duration::from_millis(750))
    {
        s.send(&ClientMsg::Join {
            version: wire::PROTOCOL,
            name: opts.name.clone(),
            password: opts.password.clone(),
        });
        s.last_join = Some(now);
    }
    if s.id.is_some()
        && !s.map_ready
        && s.last_join
            .is_none_or(|i| i.elapsed() > Duration::from_secs(1))
    {
        s.send(&ClientMsg::Join {
            version: wire::PROTOCOL,
            name: opts.name.clone(),
            password: opts.password.clone(),
        });
        s.last_join = Some(now);
    }
    if s.id.is_some()
        && s.last_ping
            .is_none_or(|(_, last)| last.elapsed() > Duration::from_secs(1))
    {
        let nonce = s.last_ping.map_or(1, |(n, _)| n + 1);
        s.last_ping = Some((nonce, now));
        s.send(&ClientMsg::Ping { nonce });
    }
    if s.id.is_some() && s.last_snapshot.is_some() && s.writer.is_none() && !s.recording_failed {
        if let Some(file) = &opts.record {
            match TraceWriter::create(file, &s.server_map, "human-client") {
                Ok(writer) => s.writer = Some(writer),
                Err(e) => {
                    s.recording_failed = true;
                    s.status = format!("Recording failed | {e}");
                    eprintln!("{}", s.status);
                }
            }
        }
    }
}

pub fn sample(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut mouse: EventReader<MouseMotion>,
    opts: Res<Options>,
    mut s: ResMut<Session>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    stamp: Option<ResMut<InputClock>>,
) {
    let Ok(mut window) = windows.get_single_mut() else {
        return;
    };
    let movement = mouse.read().map(|e| e.delta).sum::<Vec2>();
    if keys.just_pressed(KeyCode::F1) {
        s.show_help = !s.show_help;
    }
    if keys.just_pressed(KeyCode::F3) {
        s.show_debug = !s.show_debug;
    }
    let just_grabbed = buttons.just_pressed(MouseButton::Left) && !s.captured && window.focused;
    if just_grabbed {
        s.captured = true;
    }
    if keys.just_pressed(KeyCode::Escape) || !window.focused {
        s.captured = false;
    }
    let grab = if s.captured {
        CursorGrabMode::Locked
    } else {
        CursorGrabMode::None
    };
    if window.cursor_options.grab_mode != grab {
        window.cursor_options.grab_mode = grab;
        window.cursor_options.visible = !s.captured;
    }
    let live = s.captured && s.replay.is_none() && (opts.benchmark.is_none() || opts.live_input);
    if live {
        if let Some(mut stamp) = stamp {
            if movement != Vec2::ZERO
                || keys.get_just_pressed().next().is_some()
                || keys.get_just_released().next().is_some()
                || buttons.get_just_pressed().next().is_some()
                || buttons.get_just_released().next().is_some()
            {
                stamp.0 = Some(Instant::now());
            }
        }
        s.input.yaw =
            (s.input.yaw + movement.x * opts.sensitivity).rem_euclid(std::f32::consts::TAU);
        s.input.pitch = (s.input.pitch - movement.y * opts.sensitivity).clamp(-1.55, 1.55);
    }
    let active = |key| (live && keys.pressed(key)) as u8 as f32;
    s.input.forward = active(KeyCode::KeyW) - active(KeyCode::KeyS);
    s.input.strafe = active(KeyCode::KeyD) - active(KeyCode::KeyA);
    s.fire_held = live && buttons.pressed(MouseButton::Left) && !just_grabbed;
    if !live {
        s.tap_fire = false;
    }
    s.tap_fire |= live && buttons.just_pressed(MouseButton::Left) && !just_grabbed;
    s.input.fire = s.fire_held || s.tap_fire;
    // Latch one-frame presses across render frames shorter than a simulation tick.
    s.input.jump |= live && keys.just_pressed(KeyCode::Space);
    s.input.reload |= live && keys.just_pressed(KeyCode::KeyR);
    s.input.buy |= live && keys.just_pressed(KeyCode::KeyB);
    s.input.drop_bomb |= live && keys.just_pressed(KeyCode::KeyG);
    s.input.interact = live && keys.pressed(KeyCode::KeyE);
    let dead = s.me().is_some_and(|p| p.hp == 0);
    if keys.just_pressed(KeyCode::Tab) {
        s.tab_since = Some(Instant::now());
    }
    if keys.just_released(KeyCode::Tab) {
        if dead
            && s.tab_since
                .is_some_and(|at| at.elapsed() < Duration::from_millis(220))
        {
            s.next_spectator();
        }
        s.tab_since = None;
    }
    s.scoreboard = window.focused
        && keys.pressed(KeyCode::Tab)
        && (!dead
            || s.tab_since
                .is_some_and(|at| at.elapsed() >= Duration::from_millis(220)));
    if keys.just_pressed(KeyCode::F5) && matches!(s.mode, Mode::Practice | Mode::Surf) {
        if let (Some((body, yaw)), Some(life)) = (s.spawn, s.me().map(|p| p.life)) {
            s.send(&ClientMsg::Reset {
                token: s.token,
                through: s.sequence,
            });
            s.reset_life = Some((life, Instant::now(), Instant::now()));
            s.body = body;
            s.input.yaw = yaw;
            s.input.pitch = 0.;
            s.rifle = Rifle::default();
            s.pending.clear();
            s.local_shot = None;
            s.walk_distance = 0.;
            s.replay_tick = 0;
        }
    }
}

// A deterministic camera route for graphical measurements without a recorded human session.
fn benchmark_command(tick: u32) -> Input {
    let turn = (tick % 1536) as f32 / 1536. * std::f32::consts::TAU;
    Input {
        yaw: turn.sin() * 0.78,
        pitch: -0.025 + 0.045 * (turn * 2.).sin(),
        strafe: if tick % 384 < 160 {
            1.
        } else if tick % 384 < 320 {
            -1.
        } else {
            0.
        },
        fire: tick % 128 < 40,
        reload: tick % 1280 == 1100,
        ..default()
    }
}

pub fn predict(
    time: Res<Time<Real>>,
    opts: Res<Options>,
    mut session: ResMut<Session>,
    mut audio: ResMut<Sound>,
    mut stamp: Option<ResMut<InputClock>>,
) {
    let s = &mut *session;
    let Some(id) = s.id else { return };
    if !s.map_ready || s.me().is_none() {
        return;
    }
    if let Some((life, since, last)) = s.reset_life {
        if since.elapsed() < Duration::from_millis(600) {
            if last.elapsed() > Duration::from_millis(120) {
                s.send(&ClientMsg::Reset {
                    token: s.token,
                    through: s.sequence,
                });
                s.reset_life = Some((life, since, Instant::now()));
            }
            return;
        }
        s.reset_life = None;
    }
    s.accumulator += time.delta_secs().min(0.125);
    while s.accumulator >= DT {
        s.accumulator -= DT;
        s.sequence = s.sequence.wrapping_add(1);
        let automated = s.replay.is_some() || (opts.benchmark.is_some() && !opts.live_input);
        let input = if let Some(replay) = &s.replay {
            replay.input_at(0, s.replay_tick)
        } else if opts.benchmark.is_some() && !opts.live_input {
            benchmark_command(s.replay_tick)
        } else {
            s.input
        };
        if automated {
            s.input = input;
            if let Some(stamp) = &mut stamp {
                stamp.0 = Some(Instant::now());
            }
        }
        s.replay_tick = s.replay_tick.wrapping_add(1);
        let command = Command {
            seq: s.sequence,
            seen_tick: s.seen_tick(),
            input,
        };
        if let Some(writer) = &mut s.writer {
            if let Err(e) = writer.record(command.seq, id, input) {
                s.writer = None;
                s.recording_failed = true;
                s.status = format!("Recording failed | {e}");
                eprintln!("{}", s.status);
            }
        }
        s.pending.push_back(command);
        if s.pending.len() > 256 {
            s.pending.pop_front();
        }
        if s.can_move() {
            let before = s.body.pos;
            motion::step(&mut s.body, input, &s.map, &s.tuning);
            if s.body.grounded {
                s.walk_distance += (s.body.pos - before).with_y(0.).length();
                if s.walk_distance >= 1.85 {
                    s.walk_distance %= 1.85;
                    audio.emit(Cue::Step, s.body.pos, false);
                }
            }
            if s.me().is_some_and(|p| p.armed) {
                let tick = s.server_tick.wrapping_add(s.pending.len() as u32);
                let mut combat_input = input;
                combat_input.fire &= s.mode != Mode::Surf;
                if let Some(shot) = s.rifle.tick(
                    combat_input,
                    &s.body,
                    tick,
                    sendit_sim::shot_seed(id, command.seq),
                    &s.tuning,
                ) {
                    s.local_shot = Some(shot);
                    s.local_shot_time = Some(Instant::now());
                    audio.emit(Cue::Shot, s.body.pos, false);
                }
            }
        }
        s.tap_fire = false;
        s.input.fire = s.fire_held;
        s.input.jump = false;
        s.input.drop_bomb = false;
        s.input.buy = false;
        s.input.reload = false;
        let from = s.pending.len().saturating_sub(wire::INPUT_REDUNDANCY);
        s.send(&ClientMsg::Input {
            token: s.token,
            commands: s.pending.range(from..).copied().collect(),
        });
    }
}

pub fn on_exit(mut exit: EventReader<AppExit>, mut s: ResMut<Session>) {
    if exit.read().next().is_some() {
        if let Some(writer) = &mut s.writer {
            if let Err(e) = writer.flush() {
                eprintln!("unable to finish input recording: {e}");
            }
        }
        if s.id.is_some() {
            s.send(&ClientMsg::Leave { token: s.token });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authoritative_correction_replays_pending_inputs_without_replacing_local_aim() {
        let map = Arena::load("drill").unwrap();
        let mut world = sendit_sim::World::new(map, Mode::Practice, Tuning::default());
        assert!(world.add(7));
        let mut client = Session::open(&Options::default()).unwrap();
        let mut audio = Sound::new(false);
        client.id = Some(7);
        client.map_ready = true;
        client.snapshot(0, world.views(), world.round_view(), &mut audio);
        let input = Input {
            strafe: 1.,
            yaw: 0.15,
            fire: true,
            ..Input::default()
        };
        for seq in 1..=9 {
            client.pending.push_back(Command {
                seq,
                seen_tick: 0,
                input,
            });
        }
        for cmd in client.pending.iter().take(5) {
            world.advance(&[(7, *cmd)]);
        }
        client.input.yaw = 1.5;
        client.body.pos.x += 1.; // Deliberately wrong local prediction before the correction.
        client.snapshot(world.tick, world.views(), world.round_view(), &mut audio);
        assert_eq!(client.pending.len(), 4);
        assert_eq!(client.input.yaw, 1.5);
        for cmd in &client.pending {
            world.advance(&[(7, *cmd)]);
        }
        let server = world.view(7).unwrap();
        assert_eq!(client.body.pos, server.body.pos);
        assert_eq!(client.rifle, server.rifle);
    }

    #[test]
    fn life_change_discards_old_prediction_and_stale_snapshot_is_ignored() {
        let mut world = sendit_sim::World::new(
            Arena::load("drill").unwrap(),
            Mode::Practice,
            Tuning::default(),
        );
        world.add(9);
        let mut client = Session::open(&Options::default()).unwrap();
        let mut audio = Sound::new(false);
        client.id = Some(9);
        client.map_ready = true;
        let old = world.views();
        client.snapshot(0, old.clone(), world.round_view(), &mut audio);
        client.pending.push_back(Command {
            seq: 1,
            seen_tick: 0,
            input: Input {
                forward: 1.,
                ..Input::default()
            },
        });
        world.reset(9);
        world.advance(&[]);
        client.snapshot(world.tick, world.views(), world.round_view(), &mut audio);
        assert!(client.pending.is_empty());
        assert_eq!(client.body.pos, world.view(9).unwrap().body.pos);
        client.snapshot(0, old, world.round_view(), &mut audio);
        assert_eq!(client.me().unwrap().life, world.view(9).unwrap().life);
    }
}
