use super::*;
use bevy::audio::Decodable;
use rodio::source::{Spatial, UniformSourceIterator};
use sendit_sim::{map::Solid, Arena};

fn room() -> Arena {
    let mut arena = Arena::load("drill").unwrap();
    arena.boxes.clear();
    arena.ramps.clear();
    arena
}

fn at(cue: Cue, position: Vec3) -> Emission {
    Emission {
        cue,
        pos: position,
        owner: Some(2),
        spatial: true,
    }
}

// Exercise the decoder, channel conversion and spatial mixer actually used by Rodio.
fn energy(event: Emission, ear: &Transform, owner: u16, map: &Arena) -> [f64; 2] {
    let settings = mix(event, ear, Some(owner), map).unwrap();
    let sound = AudioSource {
        bytes: synthesize(event.cue, settings.color).into(),
    };
    let decoded = sound.decoder();
    let samples: Box<dyn Iterator<Item = f32>> = if settings.spatial {
        let ears = listener();
        let scale = 1. / settings.radius;
        let left = ear.transform_point(ears.left_ear_offset) * scale;
        let right = ear.transform_point(ears.right_ear_offset) * scale;
        let source = Spatial::new(
            decoded,
            (event.pos * scale).to_array(),
            left.to_array(),
            right.to_array(),
        );
        Box::new(UniformSourceIterator::new(source, 2, 24_000))
    } else {
        Box::new(UniformSourceIterator::new(decoded, 2, 24_000))
    };
    let mut channels = [0.; 2];
    for (index, sample) in samples.enumerate() {
        channels[index % 2] += f64::from(sample * settings.gain).powi(2);
    }
    channels
}

#[test]
fn gun_and_steps_pan_with_the_camera_and_self_plays_centered() {
    let room = room();
    for cue in [Cue::Shot(Weapon::M4), Cue::Step(true)] {
        let source = at(cue, Vec3::X * 8.);
        let ear = Transform::IDENTITY;
        let right = energy(source, &ear, 1, &room);
        let rotated = ear.with_rotation(Quat::from_rotation_y(std::f32::consts::PI));
        let left = energy(source, &rotated, 1, &room);
        assert!(right[1] > right[0] * 1.6 && left[0] > left[1] * 1.6);
        let own = energy(source, &rotated, 2, &room);
        assert!(own[0] > 0. && (own[0] - own[1]).abs() < own[0] * 0.001);
        // A spectated participant gets the same centered first-person treatment.
        assert_eq!(own, energy(source, &ear, 2, &room));
    }
}

#[test]
fn distance_rear_elevation_and_cover_change_the_actual_footstep() {
    let mut room = room();
    let ear = Transform::from_xyz(0., 3., 0.);
    let cue = Cue::Step(false);
    let front = at(cue, ear.translation - Vec3::Z * 5.);
    let rear = at(cue, ear.translation + Vec3::Z * 5.);
    let low = at(cue, ear.translation - Vec3::Y * 5.);
    let far = at(cue, ear.translation - Vec3::Z * 18.);
    let total = |e: [f64; 2]| e[0] + e[1];
    let clear = total(energy(front, &ear, 1, &room));
    assert!(total(energy(rear, &ear, 1, &room)) < clear * 0.80);
    assert!(total(energy(low, &ear, 1, &room)) < clear * 0.95);
    assert!(total(energy(far, &ear, 1, &room)) < clear * 0.05);
    assert!(mix(at(cue, Vec3::X * cue.range()), &ear, Some(1), &room).is_none());
    room.boxes.push(Solid {
        min: Vec3::new(-2., 0., -3.),
        max: Vec3::new(2., 5., -1.),
        ..default()
    });
    assert_eq!(
        mix(front, &ear, Some(1), &room).unwrap().color,
        Coloration::Muffled
    );
    assert!(total(energy(front, &ear, 1, &room)) < clear * 0.30);
}

#[test]
fn all_original_weapon_reports_have_distinct_decodable_waveforms() {
    let mut shots = std::collections::HashSet::new();
    for cue in Cue::all() {
        let bytes = synthesize(cue, Coloration::Clear);
        let source = AudioSource {
            bytes: bytes.clone().into(),
        };
        assert!(source.decoder().any(|sample| sample != 0));
        if matches!(cue, Cue::Shot(_)) {
            assert!(shots.insert(bytes));
        }
    }
    assert_eq!(shots.len(), 6);
}

#[test]
fn respawn_discards_old_stride_and_unplayed_pulses() {
    let step = Vec3::X * (STEP_STRIDE * 0.60);
    let mut sound = Sound::new(true);
    let shot = Cue::Shot(Weapon::Ak);
    sound.emit_actor(shot, Vec3::X * 2., 7);
    sound.emit_actor(shot, Vec3::X * 3., 7);
    assert_eq!(sound.queued(shot), 1);
    assert_eq!(sound.queue[0].pos.x, 3.);
    sound.walk(7, step, Vec3::ZERO);
    sound.reset_actor(7);
    sound.walk(7, step, Vec3::ZERO);
    assert!(sound.queue.is_empty());
    sound.walk(7, step * 2., step);
    assert_eq!(sound.queued(Cue::Step(true)), 1);
    let mut muted = Sound::new(false);
    muted.emit_actor(shot, Vec3::ZERO, 7);
    muted.walk(7, step * 2., Vec3::ZERO);
    assert!(muted.queue.is_empty());
}
