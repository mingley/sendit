use crate::net::{Impairment, Peer, PERIOD};
use glam::Vec3;
use sendit_sim::{
    motion::EYE_HEIGHT, replay::Trace, wire::ClientMsg, Arena, Input, Mode, PlayerView,
};
use serde_json::json;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

fn pace(next: &mut Instant) {
    *next += PERIOD;
    let now = Instant::now();
    if *next > now {
        thread::sleep(*next - now);
    } else if now - *next > PERIOD * 4 {
        // A suspended process cannot retroactively issue inputs for elapsed server ticks.
        *next = now;
    }
}

fn connect(
    addr: &str,
    password: &str,
    net: Impairment,
    count: usize,
    arena: Option<&Arena>,
) -> Result<Vec<Peer>, String> {
    let mut peers: Vec<Peer> = Vec::with_capacity(count);
    for slot in 0..count {
        peers.push(Peer::new(addr, net, slot)?);
        let start = Instant::now();
        let mut retry = start;
        let mut next = start;
        loop {
            let now = Instant::now();
            if now >= retry {
                peers[slot].join(&format!("probe-{:02}", slot + 1), password)?;
                retry = now + Duration::from_millis(350);
            }
            for peer in &mut peers {
                peer.poll(arena)?;
            }
            if peers[slot].own().is_ok() {
                break;
            }
            if start.elapsed() > Duration::from_secs(8) {
                return Err(format!(
                    "no welcome and player snapshot for slot {} from {addr}",
                    slot + 1
                ));
            }
            pace(&mut next);
        }
    }
    Ok(peers)
}

fn aim(from: Vec3, to: Vec3) -> Input {
    let delta = (to + Vec3::Y * 1.1) - (from + Vec3::Y * EYE_HEIGHT);
    Input {
        yaw: delta.x.atan2(-delta.z),
        pitch: delta.y.atan2(delta.x.hypot(delta.z)),
        ..Input::default()
    }
}

struct Drill {
    a: Peer,
    b: Peer,
    arena: Arena,
    clock: Instant,
    predicted_before_ack: bool,
}

impl Drill {
    fn tick(&mut self, a: Input, b: Input) -> Result<(), String> {
        let before = self.a.predicted;
        self.a.input(a, Some(&self.arena))?;
        self.b.input(b, Some(&self.arena))?;
        if let (Some(before), Some(after)) = (before, self.a.predicted) {
            if a.strafe != 0.0
                && after.pos.distance(before.pos) > 0.00001
                && self.a.own()?.ack < self.a.seq
            {
                self.predicted_before_ack = true;
            }
        }
        self.a.poll(Some(&self.arena))?;
        self.b.poll(Some(&self.arena))?;
        pace(&mut self.clock);
        self.a.poll(Some(&self.arena))?;
        self.b.poll(Some(&self.arena))?;
        Ok(())
    }

    fn idle(&mut self, ticks: u32) -> Result<(), String> {
        for _ in 0..ticks {
            self.tick(Input::default(), Input::default())?;
        }
        Ok(())
    }

    fn other(&self) -> Result<&PlayerView, String> {
        self.a
            .frame()?
            .player(self.b.id.ok_or("missing target ID")?)
            .ok_or_else(|| "missing target in attacker's view".into())
    }

    fn reset(&mut self) -> Result<(), String> {
        let lives = [self.a.own()?.life, self.b.own()?.life];
        let barriers = [self.a.seq, self.b.seq];
        for i in 0..400 {
            if i % 40 == 0 {
                for (peer, life, through) in [
                    (&mut self.a, lives[0], barriers[0]),
                    (&mut self.b, lives[1], barriers[1]),
                ] {
                    if peer.own()?.life == life {
                        peer.send(ClientMsg::Reset {
                            token: peer.token,
                            through,
                        })?;
                    }
                }
            }
            self.tick(Input::default(), Input::default())?;
            if self.a.own()?.life > lives[0] && self.b.own()?.life > lives[1] {
                self.idle(24)?;
                return Ok(());
            }
        }
        Err("instant drill reset failed to start a new life for each player".into())
    }

    fn peek(&mut self, recording: &[Input]) -> Result<f32, String> {
        self.reset()?;
        let start = self.a.own()?.body.pos;
        for &input in recording {
            self.tick(input, Input::default())?;
        }
        let end = self.a.own()?.body;
        let dx = end.pos.x - start.x;
        if dx < 0.4 || end.vel.length() > 0.1 {
            return Err(format!(
                "peek did not move and stop: dx={dx:.3}, speed={:.3}",
                end.vel.length()
            ));
        }
        let error = end
            .pos
            .distance(self.a.predicted.ok_or("no local prediction")?.pos);
        if error > 0.06 {
            return Err(format!(
                "resting prediction differs from authority by {error:.3} m"
            ));
        }
        Ok(dx)
    }

    fn cover(&mut self) -> Result<u32, String> {
        self.reset()?;
        let right = Input {
            strafe: 1.0,
            ..Input::default()
        };
        for _ in 0..300 {
            if self.a.predicted.ok_or("no predicted body")?.pos.x >= 3.65 {
                break;
            }
            self.tick(right, Input::default())?;
        }
        for _ in 0..18 {
            if self.a.predicted.ok_or("no predicted body")?.vel.x <= 0.04 {
                break;
            }
            self.tick(
                Input {
                    strafe: -1.0,
                    ..right
                },
                Input::default(),
            )?;
        }
        self.idle(64)?;
        let a = self.a.own()?.body.pos;
        let b = self.other()?.body.pos;
        let eye = a + Vec3::Y * EYE_HEIGHT;
        let ray = b + Vec3::Y * 1.1 - eye;
        if self.arena.ray_distance(eye, ray.normalize(), ray.length()) >= ray.length() - 0.05 {
            return Err(format!(
                "probe failed to reach cover between {a:?} and {b:?}"
            ));
        }
        let old_shot = self.a.own()?.shot_tick;
        let baseline_hp = self.b.own()?.hp;
        let life = self.b.own()?.life;
        for i in 0..126 {
            self.tick(
                Input {
                    fire: i < 90 && i % 40 < 3,
                    ..aim(a, b)
                },
                Input::default(),
            )?;
            if self.b.own()?.hp != baseline_hp || self.b.own()?.life != life {
                return Err("a shot through the peek wall damaged the target".into());
            }
        }
        let last_shot = self.a.own()?.shot_tick;
        if last_shot <= old_shot {
            return Err("wall check did not observe an authoritative shot".into());
        }
        Ok(last_shot)
    }

    fn hit_and_respawn(&mut self) -> Result<(u8, u8, u16), String> {
        self.reset()?;
        let shooter = self.a.own()?.body.pos;
        let hp = self.b.own()?.hp;
        let life = self.b.own()?.life;
        let old_kills = self.a.own()?.kills;
        let old_deaths = self.b.own()?.deaths;
        let mut exposed_hp = hp;
        for i in 0..180 {
            let b = self.other()?.body.pos;
            self.tick(
                Input {
                    fire: i % 55 < 3,
                    ..aim(shooter, b)
                },
                Input::default(),
            )?;
            exposed_hp = self.b.own()?.hp;
            if exposed_hp < hp {
                break;
            }
        }
        if exposed_hp == 0 || exposed_hp == hp {
            return Err(format!(
                "first exposed torso hit gave {hp} → {exposed_hp} HP"
            ));
        }

        // Track the opponent at the position and tick this client actually received.
        // Wait for motion before firing, and record damage while the target is moving.
        let mut strafe = 1.0;
        let mut moving_hit = false;
        let mut seen_death = false;
        let mut max_rewind = 0u8;
        for i in 0..640 {
            let target = self.other()?.body.pos;
            let target_x = self.b.predicted.ok_or("no predicted target")?.pos.x;
            if target_x > 1.5 {
                strafe = -1.0;
            }
            if target_x < -1.5 {
                strafe = 1.0;
            }
            let fire = i > 25 && i % 40 < 3;
            self.tick(
                Input {
                    fire,
                    ..aim(shooter, target)
                },
                Input {
                    strafe,
                    ..Input::default()
                },
            )?;
            let b = self.b.own()?;
            if b.hp < exposed_hp && b.body.vel.x.abs() > 0.4 {
                moving_hit = true;
            }
            max_rewind = max_rewind.max(self.a.own()?.rewind_ticks);
            if b.hp == 0 {
                seen_death = true;
                break;
            }
        }
        if !moving_hit || !seen_death {
            return Err(format!(
                "moving-target hit={moving_hit}, observed death={seen_death}, remaining HP={}",
                self.b.own()?.hp
            ));
        }
        for _ in 0..420 {
            self.tick(Input::default(), Input::default())?;
            if self.b.own()?.life > life
                && self.b.own()?.hp == hp
                && self.a.own()?.kills > old_kills
            {
                if self.b.own()?.deaths <= old_deaths {
                    return Err("death counter did not advance".into());
                }
                return Ok((exposed_hp, max_rewind, self.a.own()?.kills));
            }
        }
        Err("practice respawn did not restore health, advance life and credit the kill".into())
    }
}

pub fn smoke(addr: &str, password: &str, net: Impairment) -> Result<(), String> {
    let arena = Arena::load("drill")?;
    let peers = connect(addr, password, net, 2, Some(&arena))?;
    let [a, b]: [Peer; 2] = peers.try_into().map_err(|_| "missing clients")?;
    let mut sim = Drill {
        a,
        b,
        arena,
        clock: Instant::now(),
        predicted_before_ack: false,
    };
    sim.idle(30)?;
    if sim.a.map != "drill" || sim.a.mode != Mode::Practice || sim.a.frame()?.players.len() != 2 {
        return Err("smoke requires a fresh empty practice server on drill".into());
    }
    if sim.a.own()?.id == sim.b.own()?.id || sim.a.own()?.team == sim.b.own()?.team {
        return Err("expected independent sessions on opposite teams".into());
    }
    let peek: Vec<_> = (0..170)
        .map(|tick| Input {
            strafe: if tick < 36 { 1.0 } else { 0.0 },
            ..Input::default()
        })
        .collect();
    let run1 = sim.peek(&peek)?;
    let run2 = sim.peek(&peek)?;
    if !sim.predicted_before_ack {
        return Err("movement failed to respond before any server acknowledgement".into());
    }
    if (run1 - run2).abs() > 0.45 {
        return Err(format!(
            "networked peek replay drifted: {run1:.3} vs {run2:.3} m"
        ));
    }
    let blocked_shot = sim.cover()?;
    let (first_hp, max_rewind, kills) = sim.hit_and_respawn()?;
    if max_rewind > 25 || (net.latency_ms >= 20 && max_rewind == 0) {
        return Err(format!(
            "unexpected rewind {max_rewind} ticks for link profile {net:?}"
        ));
    }
    let mut corrections = std::mem::take(&mut sim.a.corrections);
    corrections.sort_by(f32::total_cmp);
    let p95 = *corrections
        .get(corrections.len().saturating_sub(1) * 95 / 100)
        .ok_or("no command ACKs measured")?;
    let drops = sim.a.drops() + sim.b.drops();
    if net.loss_pct >= 5.0 && drops == 0 {
        return Err("loss-enabled run did not exercise loss".into());
    }
    println!(
        "{}",
        json!({
            "test":"two_client_drill", "passed":true, "one_way_latency_ms":net.latency_ms,
            "jitter_ms":net.jitter_ms, "loss_percent":net.loss_pct, "loss_seed":net.seed,
            "datagrams_dropped":drops, "peek_distance_m":[run1,run2], "local_prediction_before_ack":true,
            "ack_position_error_p95_m":p95, "ack_position_error_max_m":corrections.last(), "blocked_shot_tick":blocked_shot,
            "first_hit_remaining_hp":first_hp, "moving_target_hit":true, "max_observed_rewind_ticks":max_rewind,
            "practice_respawn":true, "shooter_kills":kills
        })
    );
    Ok(())
}

pub fn load(
    addr: &str,
    password: &str,
    net: Impairment,
    players: usize,
    seconds: u32,
    replay: Option<&Path>,
) -> Result<(), String> {
    let trace = replay.map(Trace::load).transpose()?;
    let mut peers = connect(addr, password, net, players, None)?;
    if let Some(trace) = &trace {
        if trace.header.map != peers[0].map {
            return Err("recording map differs from live server".into());
        }
    }
    for p in &mut peers {
        p.poll(None)?;
    }
    let bytes = |peers: &[Peer]| {
        peers
            .iter()
            .fold((0, 0), |(tx, rx), p| (tx + p.tx_bytes, rx + p.rx_bytes))
    };
    let baseline = bytes(&peers);
    let initial_tick = peers[0].frame()?.tick;
    let start = Instant::now();
    let mut next = start;
    let mut ticks = 0u32;
    while start.elapsed() < Duration::from_secs(seconds as u64) {
        for (slot, p) in peers.iter_mut().enumerate() {
            p.poll(None)?;
            let input = trace.as_ref().map_or_else(
                || {
                    let cycle = ticks.wrapping_add(slot as u32 * 19);
                    Input {
                        strafe: if cycle % 240 < 120 { 1.0 } else { -1.0 },
                        forward: 0.3,
                        yaw: (cycle as f32 * 0.007).sin() * 1.25
                            + if slot % 2 == 0 {
                                0.0
                            } else {
                                std::f32::consts::PI
                            },
                        fire: cycle % 64 < 12,
                        jump: cycle % 196 == 0,
                        interact: cycle % 600 > 240,
                        ..Input::default()
                    }
                },
                |trace| trace.input_at(slot, ticks),
            );
            p.input(input, None)?;
            p.poll(None)?;
        }
        ticks += 1;
        pace(&mut next);
    }
    let elapsed = start.elapsed().as_secs_f64();
    let mut acks = Vec::with_capacity(players);
    for p in &mut peers {
        p.poll(None)?;
        acks.push(p.own()?.ack);
    }
    if acks.contains(&0) || peers[0].frame()?.tick == initial_tick {
        return Err(
            "one or more load clients did not exchange acknowledged input with an advancing server"
                .into(),
        );
    }
    let (tx, rx) = bytes(&peers);
    let denom = elapsed * players as f64;
    let source = trace
        .as_ref()
        .map_or_else(|| json!("synthetic"), |t| json!(t.header.source));
    println!(
        "{}",
        json!({
            "test":"udp_load", "source":source, "recorded_tracks":trace.as_ref().map_or(0, |t| t.track_count()),
            "players":players,"map":peers[0].map,"seconds":elapsed,"emitted_input_ticks_per_client":ticks,
            "server_ticks_observed":peers[0].frame()?.tick - initial_tick,
            "actual_up_payload_bytes_per_player_s":(tx-baseline.0) as f64/denom,
            "actual_down_payload_bytes_per_player_s":(rx-baseline.1) as f64/denom,
            "minimum_ack":acks.iter().min(), "ip_udp_headers_included":false
        })
    );
    Ok(())
}
