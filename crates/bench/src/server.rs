use crate::{metrics, mode, Options};
use sendit_sim::{
    bots::{BotBrain, BotNav, DEFAULT_BOT_SEED},
    replay::{Trace, TraceWriter},
    wire::{decode, encode, ClientMsg, Command, ServerMsg, INPUT_REDUNDANCY},
    Arena, Input, Mode, Phase, Tuning, Weapon, World,
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

/// Freeze the benchmark's synthetic client inputs so the UDP host and the
/// in-process simulation can be driven from exactly the same traces.
pub fn record_match(mut opts: Options) -> Result<Value, String> {
    let path = opts.required("--out")?;
    let map = opts.take("--map").unwrap_or_else(|| "relay".into());
    let count = opts.value("--players", 10usize)?;
    let ticks = opts.value("--ticks", 2048u32)?;
    opts.finish()?;
    if !(1..=32).contains(&count) || !(1..=MAX_TICKS).contains(&ticks) {
        return Err("players must be 1..32 and ticks 1..10000000".into());
    }
    Arena::load(&map)?;
    let mut writer = TraceWriter::create(&path, &map, "scripted-match-v1")
        .map_err(|e| format!("{path}: {e}"))?;
    for tick in 0..ticks {
        for slot in 0..count {
            writer
                .record(tick, slot as u16 + 1, scripted(slot, tick))
                .map_err(|e| e.to_string())?;
        }
    }
    writer.flush().map_err(|e| e.to_string())?;
    Ok(json!({"source":"scripted-match-v1","saved":path,"ticks":ticks,"tracks":count,"map":map}))
}

pub fn replay_run(arena: Arena, tuning: Tuning, trace: &Trace) -> Result<Value, String> {
    let count = trace.track_count();
    if count > 32 {
        return Err("replay command supports up to 32 recorded players".into());
    }
    let mut world = World::new(arena, Mode::Practice, tuning);
    let host_capture = trace.header.source == "server-session";
    if host_capture {
        world.tick = trace.first_tick();
    }
    let mut commands = Vec::with_capacity(count);
    let mut seeded = 0;
    for elapsed in 0..trace.duration() {
        commands.clear();
        for slot in 0..count {
            let id = trace.track_id(slot);
            if let Some(start) = trace.start_at(slot, elapsed) {
                world.remove(id);
                if !world.add_replay_player(start) {
                    return Err(format!("unable to restore player {id}"));
                }
                seeded += 1;
            }
            if let Some(cmd) = trace.command_at(slot, elapsed, world.tick) {
                if world.view(id).is_none() && !world.add(id) {
                    return Err(format!("unable to add replay slot {id}"));
                }
                commands.push((id, cmd));
            }
        }
        world.advance(&commands);
    }
    let players: Vec<_> = world
        .views()
        .iter()
        .map(|p| {
            json!({
                "id":p.id, "position":p.body.pos.to_array(), "velocity":p.body.vel.to_array(),
                "hp":p.hp, "shot_tick":p.shot_tick, "shots_fired":p.rifle.shots_fired,
                "last_shot_direction":p.shot.map(|s| s.direction.to_array()),
                "equipped":p.rifle.weapon, "active_magazine_rounds":p.rifle.ammo
            })
        })
        .collect();
    Ok(json!({
        "map":trace.header.map, "workload":trace.header.source, "ticks":trace.duration(),
        "players":players, "mode":"practice", "capture_mode":trace.header.mode,
        "friction":tuning.friction, "captured_start_states":seeded,
        "start_state":if seeded > 0 {"captured player poses and inventories"} else {"fresh match spawns"},
        "scope":"input drill; independent match events and network corrections are outside this recording"
    }))
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
    let bots = opts.value("--bots", 0usize)?;
    let skill = opts.value("--bot-skill", 0.65f32)?;
    let seed = opts.value("--bot-seed", DEFAULT_BOT_SEED)?;
    opts.finish()?;
    if !(1..=32).contains(&count) || !(1..=MAX_TICKS).contains(&ticks) || warmup > MAX_TICKS {
        return Err("players must be 1..32, ticks 1..10000000, warmup 0..10000000".into());
    }
    if bots > count || !(0.0..=1.0).contains(&skill) {
        return Err("--bots must fit the player count and --bot-skill must be in [0, 1]".into());
    }
    let clients = count - bots;
    if clients == 0 && trace.is_some() {
        return Err("at least one network slot is required to replay client input".into());
    }

    let arena = Arena::load(&map)?;
    let mut world = World::new(arena, mode, tuning);
    let first_bot = sendit_sim::wire::FIRST_BOT_ID;
    let bot_ids = first_bot..first_bot + bots as u16;
    let player_ids: Vec<_> = bot_ids.clone().chain(1..=clients as u16).collect();
    for &id in &player_ids {
        if !world.add(id) {
            return Err(format!("world rejected slot {id}"));
        }
    }
    let nav = (bots > 0).then(|| BotNav::new(&world.arena));
    let mut brains: Vec<_> = bot_ids
        .map(|id| (id, BotBrain::new(id, skill, seed)))
        .collect();
    let mut ai_samples = Vec::new();
    let mut windows = vec![VecDeque::with_capacity(INPUT_REDUNDANCY); clients];
    let mut packets = Vec::with_capacity(count);
    let mut inputs = Vec::with_capacity(count);
    let mut samples = Vec::with_capacity(ticks as usize);
    let mut traffic = Traffic::new();
    let mut started = Instant::now();
    let mut cpu_start = None;
    let mut latest_snapshot = 0;
    let mut snapshots = 0;
    let mut phase_ticks = [0u32; 5];
    let mut attacks = [0u64; Weapon::ALL.len()];
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
        if let Some(nav) = &nav {
            let start_ai = Instant::now();
            let views = world.views();
            let round = world.round_view();
            for (id, brain) in &mut brains {
                let me = world.view(*id).expect("occupied bot slot");
                let input = brain.command(
                    world.tick.wrapping_add(1),
                    mode,
                    me,
                    &views,
                    &round,
                    &world.arena,
                    nav,
                );
                inputs.push((
                    *id,
                    Command {
                        seq: tick + 1,
                        seen_tick: world.tick,
                        input,
                    },
                ));
            }
            if measuring {
                ai_samples.push(start_ai.elapsed().as_nanos() as u64);
            }
        }
        world.advance(&inputs);
        if clients > 0 && world.tick % SNAPSHOT_INTERVAL == 0 {
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
                traffic.down += bytes as u64 * clients as u64;
                snapshots += 1;
            }
        }
        if measuring {
            samples.push(begin.elapsed().as_nanos() as u64);
            let phase = match world.round_view().phase {
                Phase::Warmup => 0,
                Phase::Buy => 1,
                Phase::Live => 2,
                Phase::Planted => 3,
                Phase::Ended => 4,
            };
            phase_ticks[phase] += 1;
            for shot in player_ids
                .iter()
                .filter_map(|id| world.view(*id))
                .filter(|p| p.shot_tick == world.tick)
                .filter_map(|p| p.shot)
            {
                attacks[shot.weapon.index()] += 1;
            }
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
    let client_source = if clients == 0 {
        "no network clients"
    } else {
        "deterministic-script"
    };
    let scenario = trace.as_ref().map_or_else(||json!({"source":client_source,"seed":"slot and fixed tick"}),|t|
        json!({"source":t.header.source,"trace_tracks":t.track_count(),"trace_ticks":t.duration(),
        "fan_out":"slot modulo track count, repeated tracks offset by 97 ticks; loops at trace end"}));
    ai_samples.sort_unstable();
    let ai_ms = (!ai_samples.is_empty()).then(|| {
        json!({
            "p50":to_ms(metrics::percentile(&ai_samples,50)),
            "p95":to_ms(metrics::percentile(&ai_samples,95)),
            "p99":to_ms(metrics::percentile(&ai_samples,99)),
            "max":to_ms(*ai_samples.last().unwrap())
        })
    });
    let client_rate =
        |bytes: u64| (clients > 0).then(|| bytes as f64 / clients as f64 / simulated_seconds);
    Ok(json!({
        "benchmark":"authoritative-fixed-loop", "platform":metrics::platform(),
        "map":map,"mode":mode_name,"players":count,"workload":scenario,
        "bots":bots,"network_clients":clients,"bot_skill":skill,"bot_seed":seed,"bot_decision_work_ms":ai_ms,
        "tick_hz":TICK_HZ,"snapshot_hz":TICK_HZ/SNAPSHOT_INTERVAL,"warmup_ticks":warmup,
        "measured_ticks":ticks,"snapshots_serialized":snapshots,"simulated_seconds":simulated_seconds,
        "phase_ticks":{"warmup":phase_ticks[0],"buy":phase_ticks[1],"live":phase_ticks[2],"planted":phase_ticks[3],"ended":phase_ticks[4]},
        "rifle_shots":attacks[Weapon::M4.index()] + attacks[Weapon::Ak.index()] + attacks[Weapon::Sniper.index()],
        "attacks_by_weapon":Weapon::ALL.map(|w| json!({"weapon":w, "attacks":attacks[w.index()]})),"ending_round":world.round_view().number,"ending_score":world.round_view().score,
        "deadline_ms":to_ms(DEADLINE_NS),"work_over_deadline_ticks":missed,
        "tick_ms":{"p50":to_ms(metrics::percentile(&samples,50)),"p95":to_ms(metrics::percentile(&samples,95)),
            "p99":to_ms(metrics::percentile(&samples,99)),"max":to_ms(*samples.last().unwrap())},
        "tick_scope":"decode received input, run bot control and the authoritative world tick, then serialize any network snapshot; excludes socket and scheduler waits",
        "process_cpu_seconds_including_driver":cpu, "wall_seconds_fast_forward":wall,
        "process_cpu_pct_of_one_core_including_driver":cpu.map(|s|s/wall*100.0),
        "tick_work_pct_of_realtime_budget":(busy as f64 / 1e9)/simulated_seconds*100.0,
        "rss_kib":metrics::resident_kib("VmRSS"),"peak_rss_kib":metrics::resident_kib("VmHWM"),
        "payload_bytes_per_player_per_second":{
            "uplink":client_rate(traffic.up),"downlink":client_rate(traffic.down),
            "scope":"payload per simulated network client; up to 6 unacknowledged commands, immediate LAN acknowledgments at 32 Hz; excludes join/loss/IP headers"
        }
    }))
}

#[cfg(test)]
mod replay_tests {
    use super::*;
    use sendit_sim::{shot_seed, step, PlayerView};
    use std::path::PathBuf;

    fn temp(name: &str) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "sendit-{name}-{}-{nonce}.jsonl",
            std::process::id()
        ))
    }

    fn assert_player(report: &Value, expected: &PlayerView) {
        let p = &report["players"][0];
        assert_eq!(p["id"], expected.id);
        assert_eq!(p["position"], json!(expected.body.pos.to_array()));
        assert_eq!(p["velocity"], json!(expected.body.vel.to_array()));
        assert_eq!(p["shots_fired"], expected.rifle.shots_fired);
        assert_eq!(
            p["last_shot_direction"],
            json!(expected.shot.unwrap().direction.to_array())
        );
    }

    #[test]
    fn sparse_host_recording_preserves_command_seed_hold_expiry_and_drill_reset() {
        let path = temp("sparse");
        let tune = Tuning {
            friction: 7.2,
            shot_interval_ticks: 3,
            ..Tuning::default()
        };
        let arena = Arena::load("drill").unwrap();
        let mut host = World::new(arena.clone(), Mode::Practice, tune);
        host.tick = 800;
        assert!(host.add_team(17, 1));
        let mut writer =
            TraceWriter::create_configured(&path, "drill", "server-session", Mode::Practice, tune)
                .unwrap();
        let mut gap_shots = 0;
        for elapsed in 0..64 {
            let input_tick = matches!(elapsed, 0 | 12 | 13 | 29 | 32 | 36 | 38 | 51 | 63);
            if elapsed == 32 {
                host.reset_through(17, 175);
            }
            let previous = *host.view(17).unwrap();
            if input_tick {
                let seq = 100 + 3 * elapsed;
                let input = Input {
                    strafe: 1.,
                    fire: elapsed != 63,
                    yaw: 0.631,
                    ..Input::default()
                };
                let cmd = Command {
                    seq,
                    seen_tick: host.tick - 3,
                    input,
                };
                writer.record_command(host.tick, cmd, previous).unwrap();
                host.advance(&[(17, cmd)]);
            } else {
                host.advance(&[]);
                if host.view(17).unwrap().shot_tick == host.tick {
                    gap_shots += 1;
                }
            }
        }
        writer.flush().unwrap();
        let trace = Trace::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(gap_shots > 2);
        assert_eq!(trace.input_at(0, 8).strafe, 1.);
        assert_eq!(trace.input_at(0, 9).strafe, 0.);
        assert_eq!(trace.command_at(0, 0, 800).unwrap().seen_tick, 797);
        let report = replay_run(arena, trace.header.tuning.unwrap(), &trace).unwrap();
        assert_eq!(report["captured_start_states"], 2);
        assert_player(&report, host.view(17).unwrap());
        assert_eq!(
            report["players"][0]["shot_tick"],
            host.view(17).unwrap().shot_tick
        );
    }

    #[test]
    fn client_inputs_reproduce_nondefault_team_pose_weapon_and_sequence() {
        let path = temp("client");
        let tune = Tuning {
            friction: 8.,
            ..Tuning::default()
        };
        let arena = Arena::load("drill").unwrap();
        let mut host = World::new(arena.clone(), Mode::Practice, tune);
        assert!(host.add_team(9, 1));
        let mut local = *host.view(9).unwrap();
        local.body.pos.x += 0.72;
        local.body.vel.z = 0.9;
        local.rifle.ammo -= 4;
        let mut writer =
            TraceWriter::create_configured(&path, "drill", "test-client", Mode::Practice, tune)
                .unwrap();
        for sequence in 71..=211 {
            let input = Input {
                strafe: if sequence < 120 { 0.7 } else { -0.65 },
                yaw: 1.72 + sequence as f32 * 0.00021,
                pitch: 0.02,
                fire: sequence % 47 < 25,
                ..Input::default()
            };
            let command = Command {
                seq: sequence,
                input,
                seen_tick: sequence - 5,
            };
            writer.record_client(command, local).unwrap();
            step(&mut local.body, input, &arena, &tune);
            if let Some(shot) = local.rifle.tick(
                input,
                &local.body,
                sequence,
                shot_seed(local.id, sequence),
                &tune,
            ) {
                local.shot = Some(shot);
            }
        }
        writer.flush().unwrap();
        let trace = Trace::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(trace.header.tuning.unwrap().friction, tune.friction);
        let report = replay_run(arena, tune, &trace).unwrap();
        assert_player(&report, &local);
    }
}
