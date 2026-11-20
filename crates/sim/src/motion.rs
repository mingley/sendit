//! Fixed-tick movement shared by prediction, replay, and server authority.
//! Positions denote the centre of the player's feet. Distances are metres.
use crate::map::{Arena, Axis, Ramp};
use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

pub const DT: f32 = 1.0 / 128.0;
pub const HEIGHT: f32 = 1.80;
pub const RADIUS: f32 = 0.30;
pub const EYE_HEIGHT: f32 = 1.60;
const EPS: f32 = 0.0001;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub forward: f32,
    pub strafe: f32,
    /// Radians. Zero faces -Z, positive yaw turns right, positive pitch looks up.
    pub yaw: f32,
    pub pitch: f32,
    pub jump: bool,
    pub fire: bool,
    pub interact: bool,
    pub drop_bomb: bool,
    pub buy: bool,
    pub reload: bool,
}

impl Input {
    /// Shared behavior between a missing network command and a sparse replay.
    pub fn at_age(mut self, age: u32) -> Self {
        if age > 8 {
            return Self {
                yaw: self.yaw,
                pitch: self.pitch,
                ..Self::default()
            };
        }
        if age > 0 {
            self.buy = false;
            self.drop_bomb = false;
            self.reload = false;
        }
        self
    }

    /// Treat invalid analog values as neutral rather than poisoning replicated state.
    pub fn sanitized(mut self) -> Self {
        let finite = |value: f32| if value.is_finite() { value } else { 0.0 };
        self.forward = finite(self.forward).clamp(-1.0, 1.0);
        self.strafe = finite(self.strafe).clamp(-1.0, 1.0);
        self.yaw = finite(self.yaw) % std::f32::consts::TAU;
        self.pitch = finite(self.pitch).clamp(-1.56, 1.56);
        self
    }
}

pub fn look_direction(yaw: f32, pitch: f32) -> Vec3 {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    Vec3::new(sy * cp, sp, -cy * cp)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Tuning {
    pub run_speed: f32,
    /// Ground drag in reciprocal seconds. Below stop_speed, drag is constant.
    pub friction: f32,
    pub stop_speed: f32,
    /// Accelerations are m/s².
    pub ground_accel: f32,
    pub counter_accel: f32,
    pub air_accel: f32,
    pub air_wish_cap: f32,
    pub gravity: f32,
    pub jump_speed: f32,
    pub step_height: f32,
    /// Rise/run above which a surface has air steering and projected gravity.
    pub surf_slope: f32,
    pub accuracy_speed: f32,
    /// Angular radii in radians, independent of the fixed recoil pattern.
    pub rest_spread: f32,
    pub moving_spread: f32,
    pub air_spread: f32,
    pub burst_spread: f32,
    pub recoil_scale: f32,
    pub recoil_recovery_ticks: u16,
    pub shot_interval_ticks: u16,
    pub reload_ticks: u16,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            run_speed: 6.0,
            friction: 5.2,
            stop_speed: 2.0,
            ground_accel: 48.0,
            counter_accel: 70.0,
            air_accel: 11.0,
            air_wish_cap: 1.7,
            gravity: 18.0,
            jump_speed: 6.1,
            step_height: 0.34,
            surf_slope: 0.45,
            accuracy_speed: 0.45,
            rest_spread: 0.0,
            moving_spread: 0.020,
            air_spread: 0.033,
            burst_spread: 0.0008,
            recoil_scale: 1.0,
            recoil_recovery_ticks: 30,
            shot_interval_ticks: 13,
            reload_ticks: 220,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Body {
    pub pos: Vec3,
    pub vel: Vec3,
    pub grounded: bool,
    pub surfing: bool,
    pub jump_held: bool,
}

impl Body {
    pub fn at(pos: Vec3) -> Self {
        Self {
            pos,
            vel: Vec3::ZERO,
            grounded: false,
            surfing: false,
            jump_held: false,
        }
    }

    pub fn speed(&self) -> f32 {
        Vec2::new(self.vel.x, self.vel.z).length()
    }

    pub fn eye(&self) -> Vec3 {
        self.pos + Vec3::Y * EYE_HEIGHT
    }
}

#[derive(Clone, Copy)]
struct Support {
    height: f32,
    /// Surface gradient d(height)/d(x,z).
    slope: Vec2,
}

impl Support {
    fn flat(height: f32) -> Self {
        Self {
            height,
            slope: Vec2::ZERO,
        }
    }

    fn vertical_speed(self, velocity: Vec3) -> f32 {
        self.slope.dot(Vec2::new(velocity.x, velocity.z))
    }
}

fn ramp_surface(ramp: &Ramp, pos: Vec3) -> Support {
    let (axis, unit) = match ramp.axis {
        Axis::X => (0, Vec2::X),
        Axis::Z => (2, Vec2::Y),
    };
    let run = (ramp.max[axis] - ramp.min[axis]).max(EPS);
    let slope = (ramp.max.y - ramp.min.y) / run;
    Support {
        height: ramp.height_at(pos.x, pos.z),
        slope: unit * if ramp.rising { slope } else { -slope },
    }
}

fn over_box(pos: Vec3, min: Vec3, max: Vec3) -> bool {
    pos.x > min.x - RADIUS + EPS
        && pos.x < max.x + RADIUS - EPS
        && pos.z > min.z - RADIUS + EPS
        && pos.z < max.z + RADIUS - EPS
}

fn over_ramp(pos: Vec3, ramp: &Ramp) -> bool {
    pos.x >= ramp.min.x - EPS
        && pos.x <= ramp.max.x + EPS
        && pos.z >= ramp.min.z - EPS
        && pos.z <= ramp.max.z + EPS
}

/// Find the highest eligible support. Only the centre samples the slope, so a
/// footprint spanning two adjacent ramp meshes cannot introduce a flat ledge.
fn support(
    arena: &Arena,
    pos: Vec3,
    previous: Vec3,
    mut eligible: impl FnMut(f32, f32) -> bool,
) -> Option<Support> {
    let mut result: Option<Support> = None;
    let mut consider = |surface: Support, previous_height: f32| {
        if eligible(surface.height, previous_height)
            && result.is_none_or(|old| {
                surface.height > old.height + EPS
                    || ((surface.height - old.height).abs() <= EPS
                        && surface.slope.length_squared() > old.slope.length_squared())
            })
        {
            result = Some(surface);
        }
    };
    for solid in &arena.boxes {
        if over_box(pos, solid.min, solid.max) {
            consider(Support::flat(solid.max.y), solid.max.y);
        }
    }
    for ramp in &arena.ramps {
        if over_ramp(pos, ramp) {
            consider(ramp_surface(ramp, pos), ramp_surface(ramp, previous).height);
        }
    }
    result
}

pub(crate) fn clearance(arena: &Arena, feet: Vec3) -> bool {
    let blocked = |bottom: f32, top: f32| top > feet.y + EPS && bottom < feet.y + HEIGHT - EPS;
    !arena
        .boxes
        .iter()
        .any(|s| over_box(feet, s.min, s.max) && blocked(s.min.y, s.max.y))
        && !arena
            .ramps
            .iter()
            .any(|r| over_ramp(feet, r) && blocked(r.min.y, ramp_surface(r, feet).height))
}

fn sweep_horizontal(
    body: &mut Body,
    arena: &Arena,
    axis: usize,
    step_height: f32,
    previous: Vec3,
    floor: Option<Support>,
) {
    let start = body.pos[axis];
    let movement = body.vel[axis] * DT;
    if movement == 0.0 {
        return;
    }
    let other = 2 - axis;
    let mut end = start + movement;
    let mut clip = |min: Vec3, max: Vec3, ramp: Option<&Ramp>| {
        if body.pos[other] <= min[other] - RADIUS + EPS
            || body.pos[other] >= max[other] + RADIUS - EPS
            || body.pos.y + HEIGHT <= min.y + EPS
        {
            return;
        }
        let face = if movement > 0.0 { min[axis] } else { max[axis] };
        let boundary = face - RADIUS * movement.signum();
        if (boundary - start) * movement.signum() < -EPS
            || (end - boundary) * movement.signum() < 0.0
        {
            return;
        }
        let mut contact = body.pos.clamp(min, max);
        contact[axis] = face;
        let top = ramp.map_or(max.y, |r| ramp_surface(r, contact).height);
        if let (Some(_), Some(surface)) = (ramp, floor) {
            let offset = contact - previous;
            let old_plane = surface.height + surface.slope.dot(Vec2::new(offset.x, offset.z));
            if (old_plane - top).abs() <= EPS {
                return;
            }
        }
        let rise = top - body.pos.y;
        if rise <= EPS {
            return;
        }
        let mut stepped = body.pos;
        stepped[axis] = end;
        stepped.y = top;
        if rise <= step_height + EPS && clearance(arena, stepped) {
            return;
        }
        end = boundary;
    };
    for solid in &arena.boxes {
        clip(solid.min, solid.max, None);
    }
    for ramp in &arena.ramps {
        clip(ramp.min, ramp.max, Some(ramp));
    }
    if (end - start - movement).abs() > EPS {
        body.vel[axis] = 0.0;
    }
    body.pos[axis] = end;
}

fn ceiling(arena: &Arena, pos: Vec3, old_y: f32, new_y: f32) -> Option<f32> {
    if new_y <= old_y {
        return None;
    }
    let hit = |bottom: f32| old_y + HEIGHT <= bottom + EPS && new_y + HEIGHT >= bottom;
    let boxes = arena
        .boxes
        .iter()
        .filter(|b| over_box(pos, b.min, b.max) && hit(b.min.y))
        .map(|b| b.min.y);
    let ramps = arena
        .ramps
        .iter()
        .filter(|r| over_ramp(pos, r) && hit(r.min.y))
        .map(|r| r.min.y);
    boxes.chain(ramps).min_by(f32::total_cmp)
}

fn accelerate(velocity: &mut Vec2, wish: Vec2, max_speed: f32, acceleration: f32) {
    let strength = wish.length();
    if strength <= EPS {
        return;
    }
    let direction = wish / strength;
    let room = max_speed * strength - velocity.dot(direction);
    *velocity += direction * room.clamp(0.0, acceleration.max(0.0) * strength * DT);
}

fn attach(body: &mut Body, surface: Support, tuning: &Tuning, landing: bool) {
    body.pos.y = surface.height;
    body.surfing = surface.slope.length() >= tuning.surf_slope.max(EPS);
    body.grounded = !body.surfing;
    if landing && body.surfing {
        let normal = Vec3::new(-surface.slope.x, 1.0, -surface.slope.y).normalize();
        let into_surface = body.vel.dot(normal).min(0.0);
        body.vel -= normal * into_surface;
    }
    body.vel.y = surface.vertical_speed(body.vel);
}

/// Advance one and only one simulation tick. Camera orientation is an input;
/// no render delta, wall clock, or random source participates.
pub fn step(body: &mut Body, input: Input, arena: &Arena, tuning: &Tuning) {
    let input = input.sanitized();
    let previous = body.pos;
    let mut floor = support(arena, previous, previous, |height, _| {
        (height - previous.y).abs() <= 0.002
    });
    if !body.grounded && !body.surfing {
        // An ascending player just leaving a surface must not snap back to it.
        floor = floor.filter(|f| body.vel.y <= f.vertical_speed(body.vel) + EPS);
    }
    if let Some(f) = floor {
        attach(body, f, tuning, !body.grounded && !body.surfing);
    } else {
        body.grounded = false;
        body.surfing = false;
    }
    let jump = input.jump && !body.jump_held && body.grounded;
    body.jump_held = input.jump;
    let (s, c) = input.yaw.sin_cos();
    let wish = Vec2::new(
        s * input.forward + c * input.strafe,
        -c * input.forward + s * input.strafe,
    )
    .clamp_length_max(1.0);
    let mut planar = Vec2::new(body.vel.x, body.vel.z);
    if body.grounded && !jump {
        let speed = planar.length();
        let drag = speed.max(tuning.stop_speed) * tuning.friction.max(0.0) * DT;
        let after_drag = (speed - drag).max(0.0);
        if speed > 0.0 {
            planar *= after_drag / speed;
        }
        let accel = if wish.dot(planar) < -EPS {
            tuning.counter_accel
        } else {
            tuning.ground_accel
        };
        accelerate(&mut planar, wish, tuning.run_speed, accel);
        // Ground input cannot manufacture overspeed, but a landing retains its
        // incoming horizontal momentum and loses it to ordinary friction.
        planar = planar.clamp_length_max(tuning.run_speed.max(after_drag));
    } else {
        accelerate(&mut planar, wish, tuning.air_wish_cap, tuning.air_accel);
        if let Some(f) = floor.filter(|_| body.surfing) {
            // Gravity projected onto the tangent plane. Along-seam velocity
            // is retained regardless of how many mesh sections are crossed.
            planar -= f.slope * (tuning.gravity * DT / (1.0 + f.slope.length_squared()));
        }
    }
    body.vel.x = planar.x;
    body.vel.z = planar.y;
    if jump {
        body.vel.y = tuning.jump_speed + body.vel.y.max(0.0);
        body.grounded = false;
        floor = None;
    }
    let step_height = if body.grounded {
        tuning.step_height.max(0.0)
    } else {
        0.0
    };
    sweep_horizontal(body, arena, 0, step_height, previous, floor);
    sweep_horizontal(body, arena, 2, step_height, previous, floor);

    if let Some(old_floor) = floor {
        let dy = old_floor.vertical_speed(body.vel) * DT;
        let projected = previous.y + dy;
        let snap = tuning.step_height.max(0.0) + EPS;
        let next = support(arena, body.pos, previous, |height, _| {
            height >= projected.min(previous.y) - snap && height <= projected.max(previous.y) + snap
        });
        if let Some(next) = next {
            // Preserve takeoff when an ascending plane ends or levels out.
            if dy <= EPS || next.height >= projected - EPS {
                if ceiling(arena, body.pos, previous.y, next.height).is_some() {
                    body.pos = previous;
                    body.vel = Vec3::ZERO;
                    attach(body, old_floor, tuning, false);
                    return;
                }
                // Entering an uphill bank redirects the incoming speed onto
                // its plane. Splitting that plane into meshes costs no speed.
                let new_plane = (old_floor.slope - next.slope).length_squared() > EPS * EPS;
                attach(body, next, tuning, new_plane);
                return;
            }
        }
        body.vel.y = old_floor.vertical_speed(body.vel);
    }
    body.grounded = false;
    body.surfing = false;
    body.vel.y -= tuning.gravity * DT;
    let desired = previous.y + body.vel.y * DT;
    if let Some(bottom) = ceiling(arena, body.pos, previous.y, desired) {
        body.pos.y = bottom - HEIGHT;
        body.vel.y = 0.0;
        return;
    }
    let landing = support(arena, body.pos, previous, |height, old_height| {
        previous.y + EPS >= old_height && desired <= height + EPS
    });
    if let Some(surface) = landing {
        attach(body, surface, tuning, true);
    } else {
        body.pos.y = desired;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{Material, Solid};
    use crate::rifle::Rifle;

    fn solid(min: [f32; 3], max: [f32; 3]) -> Solid {
        Solid {
            min: min.into(),
            max: max.into(),
            material: Material::Wall,
        }
    }

    fn ground() -> Arena {
        Arena {
            boxes: vec![solid([-64., -1., -64.], [64., 0., 64.])],
            ..Arena::default()
        }
    }

    #[test]
    fn counter_strafe_recovers_accuracy_earlier_and_closer() {
        let tuning = Tuning::default();
        let arena = ground();
        let mut body = Body::at(Vec3::ZERO);
        let right = Input {
            strafe: 1.0,
            ..Input::default()
        };
        for _ in 0..64 {
            step(&mut body, right, &arena, &tuning);
        }
        assert_eq!(body.speed(), tuning.run_speed);
        let brake = |strafe| {
            let mut observed = body;
            for tick in 1..=128 {
                step(
                    &mut observed,
                    Input {
                        strafe,
                        ..Input::default()
                    },
                    &arena,
                    &tuning,
                );
                if Rifle::default().accurate(&observed, &tuning) {
                    return (tick, observed.pos.x - body.pos.x);
                }
            }
            panic!("the peek never regained accuracy");
        };
        let coast = brake(0.0);
        let counter = brake(-1.0);
        assert!(counter.0 + 8 < coast.0);
        assert!(counter.1 < coast.1 * 0.5);
        assert_eq!(brake(-1.0), counter);
    }

    #[test]
    fn steps_clear_without_bobbing_and_low_ceilings_block_climbs() {
        let mut stairs = ground();
        stairs.boxes.push(solid([0., 0., -2.], [0.9, 0.22, 2.]));
        stairs.boxes.push(solid([0.9, 0., -2.], [3., 0.44, 2.]));
        let right = Input {
            strafe: 1.0,
            ..Input::default()
        };
        let tuning = Tuning::default();
        let mut walker = Body::at(Vec3::new(-1., 0., 0.));
        for _ in 0..57 {
            step(&mut walker, right, &stairs, &tuning);
        }
        assert!(walker.pos.x > 1.);
        assert!((walker.pos.y - 0.44).abs() < 1e-5);
        assert!(walker.grounded);
        let mut low = stairs;
        low.boxes.push(solid([0., 1.90, -2.], [2., 2.4, 2.]));
        let mut blocked = Body::at(Vec3::new(-1., 0., 0.));
        for _ in 0..57 {
            step(&mut blocked, right, &low, &tuning);
        }
        assert!((blocked.pos.x + RADIUS).abs() < 0.001);
        assert_eq!(blocked.pos.y, 0.0);
    }

    #[test]
    fn high_fall_and_ceiling_contacts_use_swept_height() {
        let mut arena = ground();
        let tune = Tuning::default();
        let mut fall = Body::at(Vec3::new(0.0, 1.5, 0.0));
        fall.vel.y = -400.0;
        step(&mut fall, Input::default(), &arena, &tune);
        assert!(fall.grounded && fall.pos.y == 0.0);
        arena.boxes.push(solid([-1., 2., -1.], [1., 3., 1.]));
        let jump = Input {
            jump: true,
            ..Input::default()
        };
        for _ in 0..10 {
            step(&mut fall, jump, &arena, &tune);
            assert!(fall.pos.y + HEIGHT <= 2.0 + EPS);
        }
        assert!(!fall.grounded);
    }

    fn simulate_second(fps: u32, mut tick: impl FnMut()) {
        let mut remainder = 0;
        let mut ticks = 0;
        for _ in 0..fps {
            remainder += 128;
            while remainder >= fps {
                remainder -= fps;
                tick();
                ticks += 1;
            }
        }
        assert_eq!(ticks, 128);
    }

    #[test]
    fn drill_ramp_seam_and_exit_preserve_tangent_across_render_rates() {
        let arena = Arena::load("drill").unwrap();
        let ramp = &arena.ramps[0];
        let tune = Tuning::default();
        let run = |fps| {
            let mut body = Body::at(Vec3::new(10.2, ramp.height_at(10.2, -2.5), -2.5));
            body.vel = Vec3::new(3.0, 3.6, 12.0);
            let mut crossed = false;
            let mut exited = false;
            simulate_second(fps, || {
                step(&mut body, Input::default(), &arena, &tune);
                if (-1.9..-1.6).contains(&body.pos.z) {
                    assert!(body.surfing);
                    assert!((body.pos.y - ramp.height_at(body.pos.x, body.pos.z)).abs() < EPS);
                    crossed = true;
                }
                if body.pos.z > 5.4 {
                    assert!(!body.surfing && !body.grounded);
                    exited = true;
                }
            });
            assert!(crossed && exited);
            assert_eq!(body.vel.z, 12.0);
            body
        };
        assert_eq!(run(60), run(240));
        assert_eq!(run(73), run(144));
    }

    #[test]
    fn drill_stairs_reach_same_landing_at_60_and_240_hz() {
        let arena = Arena::load("drill").unwrap();
        let run = |fps| {
            let mut body = Body::at(Vec3::new(-9.3, 0.0, 1.5));
            let mut step_tops = Vec::new();
            simulate_second(fps, || {
                let before = body.pos.y;
                step(
                    &mut body,
                    Input {
                        forward: 1.,
                        ..Input::default()
                    },
                    &arena,
                    &Tuning::default(),
                );
                assert!(body.grounded);
                if body.pos.y > before + EPS {
                    step_tops.push(body.pos.y);
                }
            });
            assert_eq!(step_tops.len(), 8);
            assert!((body.pos.y - 1.44).abs() < EPS && body.pos.z < -3.62);
            body
        };
        assert_eq!(run(60), run(240));
    }

    #[test]
    fn rising_ramp_cannot_push_head_into_overhead_geometry() {
        let mut arena = ground();
        arena.ramps.push(Ramp {
            min: Vec3::new(0.0, 0.0, -1.0),
            max: Vec3::new(5.0, 1.0, 1.0),
            axis: Axis::X,
            rising: true,
            ..Ramp::default()
        });
        arena.boxes.push(solid([0.0, 2.0, -1.0], [5.0, 2.4, 1.0]));
        let mut body = Body::at(Vec3::new(-0.5, 0.0, 0.0));
        for _ in 0..128 {
            step(
                &mut body,
                Input {
                    strafe: 1.0,
                    ..Input::default()
                },
                &arena,
                &Tuning::default(),
            );
            assert!(body.pos.y + HEIGHT <= 2.0 + EPS);
        }
        assert!(body.pos.x > 0.8 && body.pos.x <= 1.01);
    }

    #[test]
    fn invalid_analog_input_cannot_corrupt_a_body() {
        let invalid = Input {
            yaw: f32::NAN,
            forward: f32::INFINITY,
            strafe: 100.,
            ..Input::default()
        };
        let mut body = Body::at(Vec3::ZERO);
        step(&mut body, invalid, &ground(), &Tuning::default());
        assert!(body.pos.is_finite() && body.vel.is_finite());
        assert!(body.vel.x > 0.);
    }
}
