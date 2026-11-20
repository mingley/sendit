//! Gunfire and contact marks use the hit point from the server. Prediction drives the held weapon.
use super::{models, Eye, SceneAssets, ViewModel};
use crate::game::Session;
use bevy::prelude::*;
use sendit_sim::{Arena, Mode, Shot, Weapon};
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
                ..flat(Color::srgba_u8(255, 215, 142, 145))
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

// A near miss can cross the eye. Clip the streak before perspective magnifies its width.
fn tracer(mut start: Vec3, mut end: Vec3, eye: &Transform) -> Option<Transform> {
    const NEAR: f32 = 0.85;
    let heading = *eye.forward();
    let a = (start - eye.translation).dot(heading);
    let b = (end - eye.translation).dot(heading);
    if a.max(b) <= NEAR {
        return None;
    }
    if a.min(b) < NEAR {
        let cut = start.lerp(end, (NEAR - a) / (b - a));
        if a < b {
            start = cut;
        } else {
            end = cut;
        }
    }
    let span = end - start;
    let along = span.try_normalize()?;
    let length = span.length().min(16.);
    Some(
        Transform::from_translation(start + along * length * 0.5)
            .with_rotation(Quat::from_rotation_arc(Vec3::NEG_Z, along))
            .with_scale(Vec3::new(0.007, 0.007, length)),
    )
}

// Sample the same collision surface on either side of the contact, including sloped ramps.
fn surface_normal(map: &Arena, point: Vec3, ray: Vec3) -> Vec3 {
    let tangent = ray.any_orthonormal_vector();
    let across = |axis: Vec3| {
        let start = point - ray * 0.09 + axis * 0.008;
        let distance = map.ray_distance(start, ray, 0.18);
        (distance > 0. && distance < 0.18).then_some(start + ray * distance - point)
    };
    let Some((u, v)) = across(tangent).zip(across(ray.cross(tangent))) else {
        return -ray;
    };
    let normal = u.cross(v).try_normalize().unwrap_or(-ray);
    if normal.dot(ray) > 0. {
        -normal
    } else {
        normal
    }
}

fn shot_effect(
    commands: &mut Commands,
    assets: &EffectAssets,
    map: &Arena,
    shot: Shot,
    eye: &Transform,
    held_tip: Option<Vec3>,
) {
    let direction = shot.direction;
    if !direction.is_normalized() || !shot.distance.is_finite() {
        return;
    }
    let orientation = Quat::from_rotation_arc(Vec3::NEG_Z, direction);
    let muzzle = shot.origin + orientation * Vec3::new(0.10, -0.14, -0.43);
    let impact = shot.origin + direction * shot.distance;
    if shot.weapon != Weapon::Knife {
        if held_tip.is_none() {
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
        if let Some(line) =
            tracer(held_tip.unwrap_or(muzzle), impact, eye).filter(|_| shot.distance > 1.1)
        {
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
        // Keep dust and the small contact mark on the visible side of the hit face.
        let normal = surface_normal(map, impact, direction);
        let surface = impact + normal * 0.003;
        let mark = Transform::from_translation(surface)
            .with_rotation(Quat::from_rotation_arc(Vec3::Z, normal))
            .with_scale(Vec3::new(0.044, 0.037, 0.004));
        add(
            commands,
            &assets.fleck,
            &assets.powder,
            mark,
            5.0,
            Vec3::ZERO,
            false,
        );
        let near = (eye.translation.distance(impact) / 2.).clamp(0.2, 1.);
        for i in 0..3 {
            let angle = (shot.tick % 16) as f32 + i as f32 * 2.18;
            let velocity = (normal * 0.32
                + Vec3::new(
                    angle.cos() * 0.52,
                    0.9 + i as f32 * 0.19,
                    angle.sin() * 0.52,
                ))
                * near;
            let transform = Transform::from_translation(surface)
                .with_scale(Vec3::splat((0.013 + i as f32 * 0.005) * near));
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
    eye: Query<&Transform, With<Eye>>,
    held: Query<&Transform, With<ViewModel>>,
    mut effects: Query<(Entity, &mut Transform, &mut Effect), (Without<Eye>, Without<ViewModel>)>,
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
    let (Ok(eye), Ok(held)) = (eye.get_single(), held.get_single()) else {
        return;
    };
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
                &s.map,
                shot,
                eye,
                (own || s.spectate == Some(p.id)).then(|| {
                    eye.transform_point(
                        held.transform_point(assets.weapons[shot.weapon.index()].tip),
                    )
                }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_streak_passing_the_viewer_stops_before_it_fills_the_screen() {
        for (a, b) in [
            (Vec3::Z * 3., Vec3::NEG_Z * 5.),
            (Vec3::NEG_Z * 5., Vec3::Z * 3.),
        ] {
            let line = tracer(a, b, &Transform::IDENTITY).unwrap();
            for z in [-0.5, 0.5] {
                assert!(line.transform_point(Vec3::Z * z).z <= -0.849);
            }
        }
        assert!(tracer(Vec3::Z, Vec3::X, &Transform::IDENTITY).is_none());
    }
}
