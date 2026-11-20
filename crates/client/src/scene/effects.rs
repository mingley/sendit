//! Gunfire and contact marks use the hit point from the server. Prediction drives the held weapon.
use super::{models, SceneAssets};
use crate::game::Session;
use bevy::prelude::*;
use sendit_sim::{Mode, Shot, Weapon};
use std::collections::HashMap;

pub(super) struct EffectAssets {
    pub flash: Handle<Mesh>,
    pub flame: Handle<StandardMaterial>,
    block: Handle<Mesh>,
    fleck: Handle<Mesh>,
    streak: Handle<StandardMaterial>,
    stone: Handle<StandardMaterial>,
    powder: Handle<StandardMaterial>,
}

impl EffectAssets {
    pub fn new(meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>) -> Self {
        let flat = |color| StandardMaterial {
            base_color: color,
            unlit: true,
            ..default()
        };
        Self {
            flash: meshes.add(models::flash_mesh()),
            flame: materials.add(StandardMaterial {
                cull_mode: None,
                ..flat(Color::WHITE)
            }),
            block: meshes.add(Cuboid::new(1., 1., 1.)),
            fleck: meshes.add(Sphere::new(0.5).mesh().ico(0).expect("icosahedron")),
            streak: materials.add(StandardMaterial {
                alpha_mode: AlphaMode::Blend,
                ..flat(Color::srgba_u8(255, 212, 123, 160))
            }),
            stone: materials.add(StandardMaterial {
                base_color: Color::srgb_u8(173, 152, 119),
                perceptual_roughness: 1.,
                ..default()
            }),
            powder: materials.add(flat(Color::srgb_u8(72, 64, 54))),
        }
    }
}

#[derive(Component)]
pub struct Effect {
    remaining: f32,
    duration: f32,
    velocity: Vec3,
    gravity: bool,
    scale: Vec3,
}

fn add(
    commands: &mut Commands,
    mesh: &Handle<Mesh>,
    material: &Handle<StandardMaterial>,
    transform: Transform,
    seconds: f32,
    velocity: Vec3,
    gravity: bool,
) {
    commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(material.clone()),
        transform,
        Effect {
            remaining: seconds,
            duration: seconds,
            velocity,
            gravity,
            scale: transform.scale,
        },
    ));
}

// Offsetting the mark towards the observer places it on the visible side of a clipped face.
fn shot_effect(commands: &mut Commands, assets: &EffectAssets, shot: Shot, seen_from_eye: bool) {
    let direction = shot.direction;
    if !direction.is_normalized() || !shot.distance.is_finite() {
        return;
    }
    let orientation = Quat::from_rotation_arc(Vec3::NEG_Z, direction);
    let muzzle = shot.origin + orientation * Vec3::new(0.10, -0.14, -0.43);
    let impact = shot.origin + direction * shot.distance;
    if shot.weapon != Weapon::Knife {
        if !seen_from_eye {
            let roll = (shot.tick % 31) as f32 * 0.61;
            add(
                commands,
                &assets.flash,
                &assets.flame,
                Transform::from_translation(muzzle)
                    .with_rotation(orientation * Quat::from_rotation_z(roll))
                    .with_scale(Vec3::splat(1.12)),
                0.071,
                Vec3::ZERO,
                false,
            );
        }
        if shot.distance > 1.1 {
            let start = muzzle + direction * if seen_from_eye { 0.62 } else { 0.16 };
            let ray = impact - start;
            let length = ray.length().min(19.);
            let along = ray.normalize_or_zero();
            let line = Transform::from_translation(start + along * (length * 0.5))
                .with_rotation(Quat::from_rotation_arc(Vec3::NEG_Z, along))
                .with_scale(Vec3::new(0.014, 0.014, length));
            add(
                commands,
                &assets.block,
                &assets.streak,
                line,
                0.049,
                Vec3::ZERO,
                false,
            );
        }
    }
    if shot.hit.is_none()
        && shot.distance < shot.weapon.spec().range - 0.02
        && shot.distance > 0.045
    {
        let surface = impact - direction * 0.020;
        let mark = Transform::from_translation(surface).with_scale(Vec3::splat(0.060));
        add(
            commands,
            &assets.fleck,
            &assets.powder,
            mark,
            5.0,
            Vec3::ZERO,
            false,
        );
        for i in 0..3 {
            let angle = (shot.tick % 16) as f32 + i as f32 * 2.18;
            let velocity = (-direction * 0.65
                + Vec3::new(
                    angle.cos() * 0.52,
                    0.9 + i as f32 * 0.19,
                    angle.sin() * 0.52,
                ))
                * 1.22;
            let transform = Transform::from_translation(surface)
                .with_scale(Vec3::splat(0.035 + i as f32 * 0.014));
            add(
                commands,
                &assets.fleck,
                &assets.stone,
                transform,
                0.23 + i as f32 * 0.043,
                velocity,
                true,
            );
        }
    }
}

#[derive(Default)]
pub struct ShotHistory {
    map_revision: u64,
    epochs: Option<(u16, u32)>,
    shots: HashMap<u16, (u32, u32)>,
}

pub(crate) fn combat_effects(
    mut commands: Commands,
    s: Res<Session>,
    assets: Res<SceneAssets>,
    time: Res<Time>,
    mut history: Local<ShotHistory>,
    mut effects: Query<(Entity, &mut Transform, &mut Effect)>,
) {
    let epoch = s.id.map(|id| (id, s.connection_epoch()));
    let reset = history.map_revision != s.map_revision || history.epochs != epoch;
    for (entity, mut transform, mut effect) in &mut effects {
        effect.remaining -= time.delta_secs();
        if reset || effect.remaining <= 0. {
            commands.entity(entity).despawn();
            continue;
        }
        if effect.gravity {
            effect.velocity.y -= 6.5 * time.delta_secs();
        }
        transform.translation += effect.velocity * time.delta_secs();
        let fade = (effect.remaining / (effect.duration * 0.30)).min(1.);
        transform.scale = effect.scale * fade;
    }
    if reset {
        history.map_revision = s.map_revision;
        history.epochs = epoch;
        history.shots.clear();
    }
    if s.mode == Mode::Surf || s.me().is_none() {
        return;
    }
    history
        .shots
        .retain(|id, _| s.players.iter().any(|p| p.id == *id));
    for p in &s.players {
        let own = Some(p.id) == s.id;
        let cue = if own {
            s.confirmed_local_shot
        } else {
            s.shot_presentation(p.id)
        };
        let Some((shot, at)) = cue else {
            continue;
        };
        let stamp = (p.life, shot.tick);
        if at.elapsed().as_secs_f32() > 0.19 || history.shots.get(&p.id) == Some(&stamp) {
            continue;
        }
        history.shots.insert(p.id, stamp);
        let hidden_spectator = s.mode == Mode::Competitive
            && s.spectate.is_none()
            && s.me().is_some_and(|me| me.hp == 0);
        if !hidden_spectator {
            shot_effect(
                &mut commands,
                &assets.effects,
                shot,
                own || s.spectate == Some(p.id),
            );
        }
    }
}
