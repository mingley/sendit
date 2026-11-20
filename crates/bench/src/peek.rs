use sendit_sim::{replay::TraceWriter, step, Arena, Body, Input, Rifle, Tuning};
use serde_json::{json, Value};
use std::io;

pub const BRAKE_TICK: u32 = 64;
pub const TICKS: u32 = BRAKE_TICK + 192;

pub fn input(slot: usize, tick: u32) -> Input {
    let strafe = if tick < BRAKE_TICK {
        1.0
    } else if slot == 1 {
        -1.0
    } else {
        0.0
    };
    Input {
        strafe,
        ..Input::default()
    }
}

pub fn record(path: &str) -> io::Result<()> {
    let mut trace = TraceWriter::create(path, "drill", "scripted-peek")?;
    for tick in 0..TICKS {
        if tick == BRAKE_TICK {
            trace.mark(tick, "brake")?;
        }
        for slot in 0..2 {
            trace.record(tick, slot as u16 + 1, input(slot, tick))?;
        }
    }
    trace.flush()
}

#[derive(Debug, serde::Serialize)]
struct Stop {
    speed_on_release_mps: f32,
    accurate_after_ticks: Option<u32>,
    accurate_after_ms: Option<f64>,
    stopped_after_ticks: Option<u32>,
    stopping_distance_m: Option<f32>,
}

fn stop(
    arena: &Arena,
    tuning: &Tuning,
    slot: usize,
    duration: u32,
    brake: u32,
    input_at: &impl Fn(usize, u32) -> Input,
) -> Stop {
    // Both tracks start at the same position in the marked +X drill lane.
    let mut body = Body::at(arena.spawns[0].pos);
    let mut rifle = Rifle::default();
    let mut origin = body.pos.x;
    let mut max_x = origin;
    let mut speed = 0.0;
    let mut accurate = None;
    let mut stopped = None;
    for tick in 0..duration {
        if tick == brake {
            origin = body.pos.x;
            max_x = origin;
            speed = body.vel.x;
        }
        let input = input_at(slot, tick);
        step(&mut body, input, arena, tuning);
        let _ = rifle.tick(input, &body, tick, slot as u32, tuning);
        if tick < brake {
            continue;
        }
        let elapsed = tick - brake + 1;
        if accurate.is_none() && rifle.accurate(&body, tuning) {
            accurate = Some(elapsed);
        }
        if stopped.is_none() {
            max_x = max_x.max(body.pos.x);
            // Counter-input can pass through zero between simulation ticks.
            if body.vel.x <= 0.005 {
                stopped = Some(elapsed);
            }
        }
        if accurate.is_some() && stopped.is_some() {
            break;
        }
    }
    Stop {
        speed_on_release_mps: speed,
        accurate_after_ticks: accurate,
        accurate_after_ms: accurate.map(|t| f64::from(t) * 1000.0 / 128.0),
        stopped_after_ticks: stopped,
        stopping_distance_m: stopped.map(|_| max_x - origin),
    }
}

pub fn measure(
    arena: &Arena,
    tuning: &Tuning,
    duration: u32,
    brake: u32,
    input_at: impl Fn(usize, u32) -> Input,
) -> Value {
    let a = stop(arena, tuning, 0, duration, brake, &input_at);
    let b = stop(arena, tuning, 1, duration, brake, &input_at);
    let gain = a
        .accurate_after_ticks
        .zip(b.accurate_after_ticks)
        .map(|(release, counter)| i64::from(release) - i64::from(counter));
    json!({
        "map":arena.name, "source":"scripted-peek", "tick_hz":128, "friction_per_s":tuning.friction,
        "runup_ticks":brake, "measurement_window_ticks":duration-brake,
        "release":a, "counter_strafe":b, "counter_advantage_ticks":gain,
        "counter_advantage_ms":gain.map(|ticks|ticks as f64 * 1000.0/128.0),
        "distance_definition":"maximum forward displacement before motion stops or reverses",
        "accuracy_definition":"first post-input fixed tick satisfying the rifle's settled first-shot predicate"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_peek_replays_exactly_and_counter_input_has_an_advantage() {
        let arena = Arena::load("drill").unwrap();
        let tuning = Tuning::default();
        let a = measure(&arena, &tuning, TICKS, BRAKE_TICK, input);
        assert_eq!(a, measure(&arena, &tuning, TICKS, BRAKE_TICK, input));
        assert!(a["counter_advantage_ticks"].as_i64().unwrap() > 0);
        assert!(
            a["release"]["stopping_distance_m"].as_f64().unwrap()
                > a["counter_strafe"]["stopping_distance_m"].as_f64().unwrap()
        );
        let alternate = Tuning {
            friction: tuning.friction * 1.5,
            ..tuning
        };
        let b = measure(&arena, &alternate, TICKS, BRAKE_TICK, input);
        assert_ne!(
            a["release"]["accurate_after_ticks"],
            b["release"]["accurate_after_ticks"]
        );
    }
}
