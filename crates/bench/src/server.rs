use crate::{metrics, mode, Options};
use sendit_sim::{
    replay::Trace,
    wire::{decode, encode, ClientMsg, Command, ServerMsg, INPUT_REDUNDANCY},
    Arena, Input, Mode, Tuning, World,
};
use serde_json::{json, Value};
use std::{collections::VecDeque, f32::consts::TAU, time::Instant};

const TICK_HZ: u32 = 128;
const DEADLINE_NS: u64 = 1_000_000_000 / TICK_HZ as u64;
const SNAPSHOT_INTERVAL: u32 = 4;
const MAX_TICKS: u32 = 10_000_000;

fn scripted(slot: usize, tick: u32) -> Input {
    let phase = tick.wrapping_add(slot as u32 * 73);
    // Fixed waves mix peeks, pauses, jumps, aiming changes and bursts. All choices
    // depend exclusively on the tick and slot, with no AI or world lookups.
    Input {
        strafe: match phase / 62 % 4 {
            0 => 1.0,
            2 => -1.0,
            _ => 0.0,
        },
        forward: if phase / 107 % 2 == 0 { 0.7 } else { -0.7 },
        yaw: (slot as f32 * 0.67 + (phase / 29 % 128) as f32 * 0.03) % TAU,
        pitch: ((phase / 103 % 5) as f32 - 2.0) * 0.035,
        fire: phase % 79 < 35,
        jump: phase % 313 == 13,
        interact: phase % 251 < 110,
        reload: phase % 403 == 0,
        buy: phase % 83 == 0,
        ..Input::default()
    }
}

fn occupy(world: &mut World, count: usize) -> Result<(), String> {
    for id in 1..=count as u16 {
        if !world.add(id) {
            return Err(format!("world rejected slot {id}"));
        }
    }
    Ok(())
}

pub fn replay_run(arena: Arena, tuning: Tuning, trace: &Trace) -> Result<Value, String> {
    let count = trace.track_count();
    if count > 32 {
        return Err("replay command supports up to 32 recorded players".into());
    }
    let mut world = World::new(arena, Mode::Practice, tuning);
    occupy(&mut world, count)?;
    let mut commands = Vec::with_capacity(count);
    for tick in 0..trace.duration() {
        commands.clear();
        for slot in 0..count {
            commands.push((
                slot as u16 + 1,
                Command {
                    seq: tick + 1,
                    seen_tick: world.tick,
                    input: trace.input_at(slot, tick),
                },
            ));
        }
        world.advance(&commands);
    }
    let players:Vec<_> = world.views().iter().map(|p|json!({
        "id":p.id,"position":p.body.pos.to_array(),"velocity":p.body.vel.to_array(),"hp":p.hp,"shot_tick":p.shot_tick
    })).collect();
    Ok(
        json!({"map":trace.header.map,"workload":trace.header.source,"ticks":trace.duration(),
        "players":players,"mode":"practice","start_state":"fresh match spawns; only input is recorded"}),
    )
}

struct Traffic {
    up: u64,
    down: u64,
}

impl Traffic {
    fn new() -> Self {
        Self { up: 0, down: 0 }
    }
}

pub fn run(mut opts: Options) -> Result<Value, String> {
    let trace = opts.take("--replay").map(Trace::load).transpose()?;
    let map = opts.take("--map").unwrap_or_else(|| {
        trace
            .as_ref()
            .map_or("drill", |t| &t.header.map)
            .to_string()
    });
    if trace.as_ref().is_some_and(|t| t.header.map != map) {
        return Err("replay map must match --map".into());
    }
    let mode_name = opts.take("--mode").unwrap_or_else(|| "practice".into());
    let mode = mode(&mode_name)?;
    let tuning = opts.tuning()?;
    let count = opts.value("--players", 10usize)?;
    let ticks = opts.value("--ticks", 8192u32)?;
    let warmup = opts.value("--warmup", 512u32)?;
    opts.finish()?;
    if !(1..=32).contains(&count) || !(1..=MAX_TICKS).contains(&ticks) || warmup > MAX_TICKS {
        return Err("players must be 1..32, ticks 1..10000000, warmup 0..10000000".into());
    }

    let arena = Arena::load(&map)?;
    let mut world = World::new(arena, mode, tuning);
    occupy(&mut world, count)?;
    let mut windows = vec![VecDeque::with_capacity(INPUT_REDUNDANCY); count];
    let mut packets = Vec::with_capacity(count);
    let mut inputs = Vec::with_capacity(count);
    let mut samples = Vec::with_capacity(ticks as usize);
    let mut traffic = Traffic::new();
    let mut started = Instant::now();
    let mut cpu_start = None;
    let mut latest_snapshot = 0;
    let mut snapshots = 0;
    for tick in 0..(warmup + ticks) {
        if tick == warmup {
            started = Instant::now();
            cpu_start = metrics::process_seconds();
        }
        let measuring = tick >= warmup;
        packets.clear();
        for (slot, window) in windows.iter_mut().enumerate() {
            let input = trace
                .as_ref()
                .map_or_else(|| scripted(slot, tick), |t| t.input_at(slot, tick));
            if window.len() == INPUT_REDUNDANCY {
                window.pop_front();
            }
            window.push_back(Command {
                seq: tick + 1,
                seen_tick: latest_snapshot,
                input,
            });
            let datagram = encode(&ClientMsg::Input {
                // Match the on-wire width of real 64-bit session tokens.
                token: 0xd123_70ea_0000_0000 | slot as u64,
                commands: window.iter().copied().collect(),
            })?;
            if measuring {
                traffic.up += datagram.len() as u64;
            }
            packets.push(datagram);
        }
        // Client-side input generation/encoding happens outside the per-tick timer.
        let begin = Instant::now();
        inputs.clear();
        for (slot, datagram) in packets.iter().enumerate() {
            let ClientMsg::Input { commands, .. } = decode::<ClientMsg>(datagram)? else {
                return Err("encoded input produced a different message".into());
            };
            let command = *commands.last().ok_or("empty redundant command window")?;
            inputs.push((slot as u16 + 1, command));
        }
        world.advance(&inputs);
        if world.tick % SNAPSHOT_INTERVAL == 0 {
            latest_snapshot = world.tick;
            let snapshot = ServerMsg::Snapshot {
                tick: world.tick,
                players: world.views(),
                round: world.round_view(),
            };
            let bytes = encode(&snapshot)?.len();
            // The modeled LAN acknowledges every received command in this snapshot.
            for window in &mut windows {
                window.clear();
            }
            if measuring {
                traffic.down += bytes as u64 * count as u64;
                snapshots += 1;
            }
        }
        if measuring {
            samples.push(begin.elapsed().as_nanos() as u64);
        }
    }
    let wall = started.elapsed().as_secs_f64();
    let cpu = cpu_start
        .zip(metrics::process_seconds())
        .map(|(a, b)| b - a);
    let simulated_seconds = f64::from(ticks) / f64::from(TICK_HZ);
    let missed = samples.iter().filter(|ns| **ns > DEADLINE_NS).count();
    let busy: u64 = samples.iter().sum();
    samples.sort_unstable();
    let to_ms = |n: u64| n as f64 / 1e6;
    let scenario = trace.as_ref().map_or_else(||json!({"source":"deterministic-script","seed":"slot and fixed tick"}),|t|
        json!({"source":t.header.source,"trace_tracks":t.track_count(),"trace_ticks":t.duration(),
        "fan_out":"slot modulo track count, repeated tracks offset by 97 ticks; loops at trace end"}));
    Ok(json!({
        "benchmark":"authoritative-fixed-loop", "platform":metrics::platform(),
        "map":map,"mode":mode_name,"players":count,"workload":scenario,
        "tick_hz":TICK_HZ,"snapshot_hz":TICK_HZ/SNAPSHOT_INTERVAL,"warmup_ticks":warmup,
        "measured_ticks":ticks,"snapshots_serialized":snapshots,"simulated_seconds":simulated_seconds,
        "deadline_ms":to_ms(DEADLINE_NS),"work_over_deadline_ticks":missed,
        "tick_ms":{"p50":to_ms(metrics::percentile(&samples,50)),"p95":to_ms(metrics::percentile(&samples,95)),
            "p99":to_ms(metrics::percentile(&samples,99)),"max":to_ms(*samples.last().unwrap())},
        "tick_scope":"decode received input, authoritative world tick and serialize shared snapshot; no socket or scheduler wait",
        "process_cpu_seconds_including_driver":cpu, "wall_seconds_fast_forward":wall,
        "process_cpu_pct_of_one_core_including_driver":cpu.map(|s|s/wall*100.0),
        "tick_work_pct_of_realtime_budget":(busy as f64 / 1e9)/simulated_seconds*100.0,
        "rss_kib":metrics::resident_kib("VmRSS"),"peak_rss_kib":metrics::resident_kib("VmHWM"),
        "payload_bytes_per_player_per_second":{
            "uplink":traffic.up as f64/count as f64/simulated_seconds,
            "downlink":traffic.down as f64/count as f64/simulated_seconds,
            "scope":"serialized UDP payload only; latest unacknowledged commands (up to 6), immediate LAN acknowledgments at 32 Hz; no join/loss/IP headers"
        }
    }))
}
