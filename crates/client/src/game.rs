use crate::{
    audio::{Cue, Sound},
    benchmark::InputClock,
    map_transfer::MapDownload,
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
    remote_address: String,
    local_host: Option<sendit_server::LocalMatch>,
    pub local_lobby: bool,
    pub scene_ready: bool,
    announced_player: bool,
    suppress_fire_until_release: bool,
    token: u64,
    epoch_tick: u32,
    epochs: u32,
    download: Option<MapDownload>,
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
        let socket = if opts.local_match {
            UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?
        } else {
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
            socket
        };
        socket.set_nonblocking(true).map_err(|e| e.to_string())?;
        let map = Arena::load(if opts.local_match { "aim" } else { "drill" })?;
        let spawn = map.spawns.first().ok_or("drill has no spawn")?;
        let body = Body::at(spawn.pos);
        let input = Input {
            yaw: spawn.yaw,
            ..default()
        };
        let replay = opts.replay.as_deref().map(Trace::load).transpose()?;
        Ok(Self {
            socket,
            remote_address: opts.address.clone(),
            local_host: None,
            local_lobby: opts.local_match,
            scene_ready: false,
            announced_player: false,
            suppress_fire_until_release: false,
            token: 0,
            epoch_tick: 0,
            epochs: 0,
            download: None,
            id: None,
            map,
            map_revision: 1,
            server_map: String::new(),
            map_ready: false,
            mode: if opts.local_match {
                Mode::Deathmatch
            } else {
                Mode::Practice
            },
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
            status: if opts.local_match {
                String::new()
            } else {
                format!("Connecting to {}", opts.address)
            },
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

    fn join(&mut self, opts: &Options) {
        self.send(&ClientMsg::Join {
            version: wire::PROTOCOL,
            name: opts.name.clone(),
            password: if self.local_host.is_some() {
                String::new()
            } else {
                opts.password.clone()
            },
        });
        self.last_join = Some(Instant::now());
    }

    fn start_local(&mut self, opts: &Options) -> Result<(), String> {
        if self
            .replay
            .as_ref()
            .is_some_and(|trace| trace.header.map != "aim")
        {
            return Err(
                "for this replay, join a host running its recorded map with --connect".into(),
            );
        }
        let host = sendit_server::LocalMatch::start(opts.bots, opts.bot_skill)?;
        self.socket
            .connect(host.addr())
            .map_err(|e| e.to_string())?;
        self.remote_address = host.addr().to_string();
        self.local_host = Some(host);
        self.local_lobby = false;
        self.status = "Joining local match".into();
        self.join(opts);
        println!(
            "QUICKPLAY_STARTED addr={} bots={} skill={:.2}",
            self.remote_address, opts.bots, opts.bot_skill
        );
        Ok(())
    }

    fn send(&self, message: &ClientMsg) {
        if let Ok(payload) = wire::encode(message) {
            // Inputs repeat in consecutive datagrams. A temporarily full local queue can skip one send.
            let _ = self.socket.send(&payload);
        }
    }

    fn welcome(
        &mut self,
        id: u16,
        token: u64,
        tick: u32,
        map: &str,
        mode: Mode,
        tuning: Tuning,
    ) -> Result<(), String> {
        if self.id.is_some() {
            if token == self.token {
                // An acknowledgment can repeat while a client waits for its first input slot.
                if id == self.id.unwrap() && map == self.server_map {
                    self.last_packet = Instant::now();
                }
                return Ok(());
            }
            if older(tick, self.epoch_tick) || tick == self.epoch_tick {
                return Ok(());
            }
        }
        if !sendit_sim::map::valid_map_name(map) {
            return Err("server sent an invalid map identifier".into());
        }
        if self
            .replay
            .as_ref()
            .is_some_and(|trace| trace.header.map != map)
        {
            return Err(format!(
                "input replay was recorded on a different map than {map}"
            ));
        }
        if let Some(mut writer) = self.writer.take() {
            writer
                .flush()
                .map_err(|e| format!("finish recording: {e}"))?;
        }
        self.epochs += 1;
        self.id = Some(id);
        self.token = token;
        self.epoch_tick = tick;
        self.server_tick = tick;
        self.previous_tick = tick;
        self.server_map = map.to_owned();
        self.mode = mode;
        self.tuning = tuning;
        self.players.clear();
        self.previous.clear();
        self.names.clear();
        self.pending.clear();
        self.round = None;
        self.rifle = Rifle::default();
        self.sequence = 0;
        self.accumulator = 0.;
        self.spawn = None;
        self.spectate = None;
        self.reset_life = None;
        self.last_snapshot = None;
        self.last_packet = Instant::now();
        self.last_ping = None;
        self.rtt_ms = None;
        self.local_shot = None;
        self.local_shot_time = None;
        self.hit_at = None;
        self.damage_at = None;
        self.rejected = false;
        self.replay_tick = 0;
        self.map_ready = sendit_sim::map::is_builtin(map);
        self.download = if self.map_ready {
            if self.epochs != 1 || self.map.name != map {
                self.map = Arena::load(map)?;
                self.map_revision += 1;
            }
            self.status = "Waiting for match state".into();
            None
        } else {
            self.status = format!("Downloading map  {map}");
            Some(MapDownload::new(map, token))
        };
        Ok(())
    }

    pub fn me(&self) -> Option<&PlayerView> {
        self.players.iter().find(|v| Some(v.id) == self.id)
    }

    fn spawn_pitch(&self) -> f32 {
        if self.mode != Mode::Surf {
            return 0.;
        }
        self.map
            .course
            .get(1)
            .zip(self.map.course.first())
            .map_or(0., |(next, start)| {
                let slope = *next - *start;
                slope.y.atan2(slope.xz().length()).clamp(-0.8, 0.35)
            })
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
        if self.mode != Mode::Competitive {
            self.spectate = None;
            return;
        }
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
                    self.server_tick.wrapping_add(index as u32 + 1),
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
        if self.id.is_none()
            || older(tick, self.epoch_tick)
            || (self.last_snapshot.is_some() && !older(self.server_tick, tick))
        {
            return;
        }
        self.last_packet = Instant::now();
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
        if self.local_host.is_some() && !self.announced_player {
            println!(
                "QUICKPLAY_PLAYING id={} players={}",
                self.id.unwrap(),
                self.players.len()
            );
            self.announced_player = true;
        }
        let changed_life = old_self.is_none_or(|p| p.0 != life);
        if changed_life {
            self.spawn = Some((body, yaw));
            self.input.yaw = yaw;
            self.input.pitch = self.spawn_pitch();
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
        if (self.sequence == 0 && self.pending.is_empty()) || older(self.sequence, ack) {
            self.sequence = ack;
        }
        while self.pending.front().is_some_and(|c| !older(ack, c.seq)) {
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
    mut bytes: Local<Vec<u8>>,
) {
    if s.local_lobby {
        return;
    }
    let now = Instant::now();
    if bytes.is_empty() {
        bytes.resize(wire::MAX_PACKET + 1, 0u8);
    }
    for _ in 0..64 {
        let len = match s.socket.recv(&mut bytes) {
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) => {
                if s.id.is_none() {
                    s.status = format!("Waiting for {} | {e}", s.remote_address)
                };
                break;
            }
        };
        let Ok(message) = wire::decode::<ServerMsg>(&bytes[..len]) else {
            continue;
        };
        match message {
            ServerMsg::Welcome {
                id,
                token,
                tick,
                map,
                mode,
                tuning,
            } => {
                if let Err(e) = s.welcome(id, token, tick, &map, mode, tuning) {
                    eprintln!("{e}");
                    exit.send(AppExit::error());
                }
            }
            ServerMsg::MapChunk {
                token,
                map,
                part,
                count,
                data,
            } => {
                if token != s.token || map != s.server_map {
                    continue;
                }
                let Some(download) = &mut s.download else {
                    continue;
                };
                match download.accept(token, &map, part, count, data) {
                    Ok(Some(arena)) => {
                        s.map = arena;
                        s.map_ready = true;
                        s.map_revision += 1;
                        s.download = None;
                        s.last_packet = now;
                        s.status = "Waiting for match state".into();
                        s.replay_pending();
                        println!(
                            "MAP_READY {} solids={} ramps={}",
                            s.server_map,
                            s.map.boxes.len(),
                            s.map.ramps.len()
                        );
                    }
                    Ok(None) => {
                        s.last_packet = now;
                    }
                    Err(e) => {
                        s.download = None;
                        s.rejected = true;
                        s.status = format!("Map rejected | {e}");
                        eprintln!("{}", s.status);
                        if opts.benchmark.is_some() {
                            exit.send(AppExit::error());
                        }
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
                s.id = None;
                s.download = None;
                s.map_ready = false;
                s.players.clear();
                s.pending.clear();
                if opts.benchmark.is_some() {
                    exit.send(AppExit::error());
                }
            }
            ServerMsg::Challenge { token } => {
                if s.id.is_none() && !s.rejected {
                    s.send(&ClientMsg::Confirm { token });
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
        s.download = None;
        s.names.clear();
        s.map_ready = false;
        s.players.clear();
        s.pending.clear();
        s.last_snapshot = None;
        s.status = format!("Reconnecting to {}", s.remote_address);
    }
    if s.id.is_none()
        && !s.rejected
        && s.last_join
            .is_none_or(|i| i.elapsed() > Duration::from_millis(750))
    {
        s.join(&opts);
    }
    if let Some(download) = &mut s.download {
        let requests = download.requests(now);
        for part in requests {
            s.send(&ClientMsg::MapRequest {
                token: s.token,
                part,
            });
        }
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
            // Keep a separate trace per authoritative world, including same-map restarts.
            let destination = if s.epochs <= 1 {
                file.clone()
            } else {
                let stem = file.file_stem().unwrap_or_default().to_string_lossy();
                let ext = file.extension().unwrap_or_default().to_string_lossy();
                let mut name = format!("{stem}.{}-{}", s.server_map, s.epochs);
                if !ext.is_empty() {
                    name.push('.');
                    name.push_str(&ext);
                }
                file.with_file_name(name)
            };
            match TraceWriter::create_configured(
                &destination,
                &s.server_map,
                "human-client",
                s.mode,
                s.tuning,
            ) {
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
    mut keyboard_events: EventReader<bevy::input::keyboard::KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut mouse: EventReader<MouseMotion>,
    mut opts: ResMut<Options>,
    ready: Option<Res<crate::LobbyRender>>,
    mut exit: EventWriter<AppExit>,
    mut s: ResMut<Session>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    stamp: Option<ResMut<InputClock>>,
) {
    let Ok(mut window) = windows.get_single_mut() else {
        return;
    };
    let movement = mouse.read().map(|e| e.delta).sum::<Vec2>();
    if s.local_lobby {
        use bevy::input::keyboard::Key;
        for key in keyboard_events
            .read()
            .filter(|key| window.focused && key.state.is_pressed() && !key.repeat)
        {
            let logical = match key.logical_key {
                Key::ArrowLeft => KeyCode::ArrowLeft,
                Key::ArrowRight => KeyCode::ArrowRight,
                Key::ArrowDown => KeyCode::ArrowDown,
                Key::ArrowUp => KeyCode::ArrowUp,
                _ => key.key_code,
            };
            match logical {
                KeyCode::ArrowLeft => opts.bots = opts.bots.saturating_sub(1).max(1),
                KeyCode::ArrowRight => opts.bots = (opts.bots + 1).min(9),
                KeyCode::ArrowUp => {
                    opts.bot_skill = ((opts.bot_skill * 10.).round() + 1.).min(10.) / 10.
                }
                KeyCode::ArrowDown => {
                    opts.bot_skill = ((opts.bot_skill * 10.).round() - 1.).max(0.) / 10.
                }
                _ => (),
            }
        }
        if !ready.is_some_and(|flag| flag.click_ready()) {
            return;
        }
        let automated = opts.replay.is_some() || (opts.benchmark.is_some() && !opts.live_input);
        if !automated && (!window.focused || !buttons.just_pressed(MouseButton::Left)) {
            return;
        }
        if let Err(e) = s.start_local(&opts) {
            s.status = format!("Could not start local match | {e}");
            eprintln!("{}", s.status);
            if automated {
                exit.send(AppExit::error());
            }
            return;
        }
    }
    if keys.just_pressed(KeyCode::F1) {
        s.show_help = !s.show_help;
    }
    if keys.just_pressed(KeyCode::F3) {
        s.show_debug = !s.show_debug;
    }
    let just_grabbed = buttons.just_pressed(MouseButton::Left) && !s.captured && window.focused;
    if just_grabbed {
        s.captured = true;
        s.suppress_fire_until_release = true;
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
    if live && !just_grabbed {
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
    if !buttons.pressed(MouseButton::Left) {
        s.suppress_fire_until_release = false;
    }
    s.fire_held = live && buttons.pressed(MouseButton::Left) && !s.suppress_fire_until_release;
    if !live {
        s.tap_fire = false;
    }
    s.tap_fire |= live
        && buttons.just_pressed(MouseButton::Left)
        && !just_grabbed
        && !s.suppress_fire_until_release;
    s.input.fire = s.fire_held || s.tap_fire;
    // Latch one-frame presses across render frames shorter than a simulation tick.
    s.input.jump |= live && keys.just_pressed(KeyCode::Space);
    s.input.reload |= live && keys.just_pressed(KeyCode::KeyR);
    s.input.buy |= live && keys.just_pressed(KeyCode::KeyB);
    s.input.drop_bomb |= live && keys.just_pressed(KeyCode::KeyG);
    s.input.interact = live && keys.pressed(KeyCode::KeyE);
    let dead = s.mode == Mode::Competitive && s.me().is_some_and(|p| p.hp == 0);
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
            s.input.pitch = s.spawn_pitch();
            s.rifle = Rifle::default();
            s.pending.clear();
            s.local_shot = None;
            s.walk_distance = 0.;
            s.replay_tick = 0;
        }
    }
}

fn older(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
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
    if !s.map_ready {
        return;
    }
    let Some(me) = s.me().copied() else { return };
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
            let state = PlayerView {
                body: s.body,
                rifle: s.rifle,
                ..me
            };
            if let Err(e) = writer.record_client(command, state) {
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
                if s.walk_distance >= sendit_sim::bots::STEP_STRIDE {
                    s.walk_distance %= sendit_sim::bots::STEP_STRIDE;
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
        drop(s.local_host.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_menu_waits_for_a_new_click_and_holding_capture_never_fires() {
        let opts = Options {
            bots: 1,
            ..Options::default()
        };
        let mut app = App::new();
        app.insert_resource(Session::open(&opts).unwrap())
            .insert_resource(opts)
            .init_resource::<crate::LobbyRender>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_event::<MouseMotion>()
            .add_event::<bevy::input::keyboard::KeyboardInput>()
            .add_event::<AppExit>()
            .add_systems(Update, sample);
        app.world_mut().spawn((
            Window {
                focused: true,
                ..default()
            },
            PrimaryWindow,
        ));

        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        assert!(app.world().resource::<Session>().local_lobby);
        assert!(app.world().resource::<Session>().local_host.is_none());
        app.world_mut()
            .resource_mut::<crate::LobbyRender>()
            .0
            .store(2, std::sync::atomic::Ordering::Release);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        app.update(); // A button held from loading does not start play.
        assert!(app.world().resource::<Session>().local_lobby);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Left);
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        assert!(!app.world().resource::<Session>().local_lobby);
        assert!(app.world().resource::<Session>().captured);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        app.update();
        assert!(!app.world().resource::<Session>().input.fire);

        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Left);
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        assert!(app.world().resource::<Session>().input.fire);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        assert!(!app.world().resource::<Session>().captured);
        assert!(!app.world().resource::<Session>().local_lobby);
    }

    #[test]
    fn authoritative_correction_replays_pending_inputs_without_replacing_local_aim() {
        let map = Arena::load("drill").unwrap();
        let mut world = sendit_sim::World::new(map, Mode::Practice, Tuning::default());
        assert!(world.add(7));
        let mut client = Session::open(&Options {
            local_match: false,
            ..Options::default()
        })
        .unwrap();
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
    fn rotation_to_same_custom_map_rejects_old_welcomes_and_snapshots() {
        let mut client = Session::open(&Options {
            local_match: false,
            ..Options::default()
        })
        .unwrap();
        let tuning = Tuning::default();
        client
            .welcome(2, 11, 100, "custom-arena", Mode::Practice, tuning)
            .unwrap();
        assert!(!client.map_ready);
        assert!(client.download.is_some());
        client.pending.push_back(Command::default());
        client
            .welcome(2, 22, 200, "custom-arena", Mode::Deathmatch, tuning)
            .unwrap();
        assert_eq!(client.epochs, 2);
        assert!(client.pending.is_empty());
        assert!(client.download.is_some());
        let mut audio = Sound::new(false);
        client.snapshot(199, vec![], RoundView::default(), &mut audio);
        assert!(client.last_snapshot.is_none());
        client
            .welcome(2, 11, 100, "custom-arena", Mode::Practice, tuning)
            .unwrap();
        assert_eq!((client.token, client.mode), (22, Mode::Deathmatch));
        client.snapshot(200, vec![], RoundView::default(), &mut audio);
        assert!(client.last_snapshot.is_some());
        client.id = None; // Existing endpoint timed out; a fresh server can have a lower tick.
        client
            .welcome(8, 90, 3, "drill", Mode::Practice, tuning)
            .unwrap();
        assert_eq!(client.epoch_tick, 3);
        assert!(client.map_ready);
    }

    #[test]
    fn life_change_discards_old_prediction_and_stale_snapshot_is_ignored() {
        let mut world = sendit_sim::World::new(
            Arena::load("drill").unwrap(),
            Mode::Practice,
            Tuning::default(),
        );
        world.add(9);
        let mut client = Session::open(&Options {
            local_match: false,
            ..Options::default()
        })
        .unwrap();
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
