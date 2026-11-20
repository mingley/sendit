//! Drive the shipped course from a real spawn with only the forward control.
//! Rebatch identical fixed ticks into independently paced virtual render frames.
use sendit_sim::{map::Material, step, Arena, Body, Input, Tuning, TICK_RATE};
use serde_json::{json, Value};

const CADENCES: [u32; 4] = [30, 60, 144, 240];

fn ride(arena: &Arena, stripe_z: f32, hz: u32) -> Result<Vec<Body>, String> {
    let spawn = arena.spawns.first().ok_or("surf course has no start")?;
    let input = Input {
        forward: 1.0,
        yaw: spawn.yaw,
        ..Input::default()
    };
    let mut body = Body::at(spawn.pos);
    let tune = Tuning::default();
    let mut path = vec![body];
    let mut accumulator = 0;
    for _ in 0..hz * 12 {
        // Rational arithmetic makes each simulated second contain exactly
        // 128 physics steps even for render rates that do not divide 128.
        accumulator += TICK_RATE;
        while accumulator >= hz {
            accumulator -= hz;
            step(&mut body, input, arena, &tune);
            path.push(body);
            if body.grounded && body.pos.z <= stripe_z {
                return Ok(path);
            }
            if body.pos.y < -40. {
                return Err(format!(
                    "course run fell at tick {}: {:?}",
                    path.len() - 1,
                    body.pos
                ));
            }
        }
    }
    Err(format!(
        "{hz} Hz run did not reach the finish stripe; ended at {:?}",
        body.pos
    ))
}

fn event(
    path: &[Body],
    start: usize,
    name: &str,
    find: impl Fn(Body, Body) -> bool,
) -> Result<usize, String> {
    (start.max(1)..path.len())
        .find(|&i| find(path[i - 1], path[i]))
        .ok_or_else(|| format!("missing course event: {name}"))
}

fn readout(path: &[Body], tick: usize) -> Value {
    let p = path[tick];
    json!({"tick":tick, "position_m":p.pos.to_array(), "velocity_m_s":p.vel.to_array(),
        "horizontal_speed_m_s":p.speed(), "speed_m_s":p.vel.length()})
}

pub fn measure() -> Result<Value, String> {
    let arena = Arena::load("surf")?;
    let stripe = arena
        .boxes
        .iter()
        .find(|b| b.material == Material::SiteA)
        .ok_or("course finish stripe is missing")?;
    let seam_z = arena
        .ramps
        .first()
        .ok_or("course first bank is missing")?
        .min
        .z;
    let stripe_z = (stripe.min.z + stripe.max.z) * 0.5;
    let path = ride(&arena, stripe_z, CADENCES[0])?;
    for hz in CADENCES.iter().copied().skip(1) {
        if ride(&arena, stripe_z, hz)? != path {
            return Err(format!(
                "{hz} Hz followed a different fixed-tick trajectory"
            ));
        }
    }
    let seam = event(&path, 1, "first bank seam", |a, b| {
        a.pos.z >= seam_z && b.pos.z < seam_z
    })?;
    let exit = |a: Body, b: Body| a.surfing && !b.surfing && !b.grounded;
    let takeoff = event(&path, seam + 1, "first takeoff", exit)?;
    let catch = event(&path, takeoff + 1, "receiving bank landing", |a, b| {
        !a.surfing && !a.grounded && b.surfing
    })?;
    let receiving = arena
        .ramps
        .get(2)
        .ok_or("course receiving bank is missing")?;
    let lower_seam = event(&path, catch + 1, "receiving bank seam", |a, b| {
        a.pos.z >= receiving.min.z && b.pos.z < receiving.min.z
    })?;
    for (i, bank) in [(seam, &arena.ramps[0]), (lower_seam, receiving)] {
        let rise = (bank.max.y - bank.min.y) / (bank.max.z - bank.min.z);
        let gain = Tuning::default().gravity * rise / (1.0 + rise * rise) / TICK_RATE as f32;
        if !path[i - 1].surfing
            || !path[i].surfing
            || (path[i].vel.z - (path[i - 1].vel.z - gain)).abs() > 0.0002
        {
            return Err(format!(
                "lost contact or accumulated an extra impulse at seam tick {i}"
            ));
        }
    }
    let launch = event(&path, catch + 1, "second takeoff", exit)?;
    let deck = event(&path, launch + 1, "finish deck landing", |a, b| {
        !a.surfing && !a.grounded && b.grounded
    })?;
    for tick in [takeoff, launch] {
        if path[tick].speed() + 0.001 < path[tick - 1].speed() || path[tick].vel.y >= 0.0 {
            return Err(format!("lost the bank's exit velocity at tick {tick}"));
        }
    }
    let after_landing = path
        .get(deck + 1)
        .ok_or("finish stripe must follow the deck landing")?;
    let expected_speed = path[deck].speed() * (1.0 - Tuning::default().friction / TICK_RATE as f32);
    if (after_landing.speed() - expected_speed).abs() > 0.001 {
        return Err("landing lost more horizontal speed than one tick of ground friction".into());
    }
    Ok(json!({
        "map":"surf", "workload":"forward held from first spawn through the finish stripe",
        "mode":"surf", "simulation_hz":TICK_RATE, "render_cadences_hz":CADENCES,
        "identical_states_each_tick":true, "finish_time_s":(path.len()-1) as f32 / TICK_RATE as f32,
        "before_seam":readout(&path,seam-1), "after_seam":readout(&path,seam),
        "before_first_exit":readout(&path,takeoff-1), "first_airborne":readout(&path,takeoff),
        "receiver_landing":readout(&path,catch), "before_receiving_seam":readout(&path,lower_seam-1),
        "after_receiving_seam":readout(&path,lower_seam), "second_airborne":readout(&path,launch),
        "deck_landing":readout(&path,deck), "first_ground_tick_after_landing":readout(&path,deck+1),
        "finish_stripe":readout(&path,path.len()-1)
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn shipped_surf_course_completes_both_airborne_transfers_at_four_cadences() {
        let result = super::measure().unwrap();
        assert!(result["identical_states_each_tick"].as_bool().unwrap());
        let landing = &result["deck_landing"];
        let after = &result["first_ground_tick_after_landing"];
        assert!(landing["velocity_m_s"][2].as_f64().unwrap() < -6.);
        assert!(
            after["horizontal_speed_m_s"].as_f64().unwrap()
                > 0.9 * landing["horizontal_speed_m_s"].as_f64().unwrap()
        );
    }
}
