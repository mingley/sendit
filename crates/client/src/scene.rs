//! First-person presentation and interpolated world actors.
mod effects;
pub(crate) mod environment;
mod models;

pub(crate) use effects::combat_effects;
pub(crate) use environment::{setup as setup_environment, sync_map};

use crate::game::Session;
use bevy::{core_pipeline::tonemapping::Tonemapping, prelude::*};
use models::{Uniform, WeaponArt};
use sendit_sim::{motion::EYE_HEIGHT, BombView, Mode, Weapon};
use std::collections::HashMap;

#[derive(Component)]
pub struct Eye;
#[derive(Component)]
pub struct ViewModel;
#[derive(Component)]
pub struct Remote;
#[derive(Component)]
pub struct Objective;
#[derive(Component)]
pub enum HeldPart {
    Body,
    Magazine,
    Flash,
}
#[derive(Component)]
pub enum Joint {
    Leg(u16, f32),
    Hand(u16),
}
#[derive(Resource)]
pub struct SceneAssets {
    weapons: [WeaponArt; 6],
    uniforms: [Uniform; 2],
    paint: Handle<StandardMaterial>,
    effects: effects::EffectAssets,
}

pub fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let weapons = Weapon::ALL.map(|weapon| WeaponArt::new(weapon, &mut meshes));
    let uniforms = [false, true].map(|blue| Uniform::new(blue, &mut meshes));
    let paint = mats.add(StandardMaterial {
        base_color_texture: Some(images.add(models::surface_paint())),
        perceptual_roughness: 0.91,
        reflectance: 0.14,
        ..default()
    });
    let effects = effects::EffectAssets::new(&mut meshes, &mut mats);
    let initial = &weapons[Weapon::M4.index()];
    commands
        .spawn((
            Camera3d::default(),
            Eye,
            Tonemapping::None,
            Msaa::Off,
            crate::audio::listener(),
            Projection::Perspective(PerspectiveProjection {
                fov: 73f32.to_radians(),
                near: 0.025,
                far: 320.,
                ..default()
            }),
            Transform::default(),
        ))
        .with_children(|camera| {
            camera
                .spawn((ViewModel, Transform::default(), Visibility::Hidden))
                .with_children(|root| {
                    root.spawn((
                        HeldPart::Body,
                        Mesh3d(initial.first_person.clone()),
                        MeshMaterial3d(paint.clone()),
                    ));
                    root.spawn((
                        HeldPart::Magazine,
                        Mesh3d(initial.feed.clone().unwrap_or_default()),
                        MeshMaterial3d(paint.clone()),
                    ));
                    root.spawn((
                        HeldPart::Flash,
                        Mesh3d(effects.flash.clone()),
                        MeshMaterial3d(effects.flame.clone()),
                        Visibility::Hidden,
                    ));
                });
        });
    commands.spawn((
        DirectionalLight {
            illuminance: 7000.,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(-4., 9., 5.).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    let pack = mats.add(Color::srgb_u8(161, 123, 54));
    let band = mats.add(Color::srgb_u8(55, 56, 47));
    commands
        .spawn((Objective, Transform::default(), Visibility::Hidden))
        .with_children(|root| {
            root.spawn((
                Mesh3d(meshes.add(Cuboid::new(0.36, 0.20, 0.28))),
                MeshMaterial3d(pack),
                Transform::from_xyz(0., 0.10, 0.),
            ));
            let strap = meshes.add(Cuboid::new(0.032, 0.204, 0.284));
            for x in [-0.103, 0.103] {
                root.spawn((
                    Mesh3d(strap.clone()),
                    MeshMaterial3d(band.clone()),
                    Transform::from_xyz(x, 0.102, 0.),
                ));
            }
        });
    commands.insert_resource(SceneAssets {
        weapons,
        uniforms,
        paint,
        effects,
    });
}

fn blue_uniform(mode: Mode, team: u8) -> usize {
    usize::from(mode != Mode::Deathmatch && team != 0)
}

pub fn sync_players(
    mut commands: Commands,
    s: Res<Session>,
    assets: Res<SceneAssets>,
    time: Res<Time>,
    mut roots: Query<(&mut Transform, &mut Visibility, &mut Mesh3d), With<Remote>>,
    mut joints: Query<(&Joint, &mut Transform, &mut Visibility, &mut Mesh3d), Without<Remote>>,
    mut objectives: Query<
        (&mut Transform, &mut Visibility),
        (With<Objective>, Without<Remote>, Without<Joint>),
    >,
    mut entities: Local<HashMap<u16, Entity>>,
) {
    entities.retain(|id, entity| {
        let keep = s.players.iter().any(|p| p.id == *id && Some(p.id) != s.id);
        if !keep {
            commands.entity(*entity).despawn_recursive();
        }
        keep
    });
    let waiting =
        s.mode == Mode::Competitive && s.spectate.is_none() && s.me().is_some_and(|p| p.hp == 0);
    for p in s.players.iter().filter(|v| Some(v.id) != s.id) {
        let (pos, yaw) = s.remote_pose(p);
        let transform = Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(-yaw));
        let visibility = if p.hp == 0 || s.spectate == Some(p.id) || waiting {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        let uniform = &assets.uniforms[blue_uniform(s.mode, p.team)];
        let carried = &assets.weapons[p.rifle.weapon.index()].carried;
        if let Some(entity) = entities.get(&p.id) {
            if let Ok((mut t, mut v, mut mesh)) = roots.get_mut(*entity) {
                *t = transform;
                v.set_if_neq(visibility);
                mesh.set_if_neq(Mesh3d(uniform.torso.clone()));
            }
        } else {
            let entity = commands
                .spawn((
                    Remote,
                    Mesh3d(uniform.torso.clone()),
                    MeshMaterial3d(assets.paint.clone()),
                    transform,
                    visibility,
                ))
                .with_children(|root| {
                    for side in [-1., 1.] {
                        root.spawn((
                            Joint::Leg(p.id, side),
                            Mesh3d(uniform.leg.clone()),
                            MeshMaterial3d(assets.paint.clone()),
                            Transform::from_xyz(side * 0.105, 0.829, 0.),
                        ));
                    }
                    root.spawn((
                        Joint::Hand(p.id),
                        Mesh3d(carried.clone()),
                        MeshMaterial3d(assets.paint.clone()),
                        Transform::from_xyz(0.128, 1.245, -0.18).with_scale(Vec3::splat(0.81)),
                        if p.armed && s.mode != Mode::Surf {
                            Visibility::Inherited
                        } else {
                            Visibility::Hidden
                        },
                    ));
                })
                .id();
            entities.insert(p.id, entity);
        }
    }
    for (joint, mut t, mut visible, mut mesh) in &mut joints {
        let id = match *joint {
            Joint::Leg(id, _) | Joint::Hand(id) => id,
        };
        let Some(p) = s.players.iter().find(|p| p.id == id) else {
            continue;
        };
        match *joint {
            Joint::Leg(_, side) => {
                let uniform = &assets.uniforms[blue_uniform(s.mode, p.team)];
                mesh.set_if_neq(Mesh3d(uniform.leg.clone()));
                let stride = if p.body.grounded {
                    (p.body.vel.xz().length() * 0.14).min(0.76)
                } else {
                    0.14
                };
                t.rotation = Quat::from_rotation_x(
                    side * stride * (time.elapsed_secs() * 9.8 + id as f32).sin(),
                );
            }
            Joint::Hand(_) => {
                visible.set_if_neq(if p.armed && s.mode != Mode::Surf {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                });
                mesh.set_if_neq(Mesh3d(
                    assets.weapons[p.rifle.weapon.index()].carried.clone(),
                ));
                t.rotation = Quat::from_rotation_x(p.pitch.clamp(-0.95, 1.05) * 0.8);
            }
        }
    }
    if let Ok((mut transform, mut visibility)) = objectives.get_single_mut() {
        let position = s.round.as_ref().and_then(|r| match r.bomb {
            BombView::Dropped(pos) | BombView::Planted(_, pos) => Some(pos),
            _ => None,
        });
        if let Some(pos) = position {
            transform.translation = pos;
            visibility.set_if_neq(Visibility::Inherited);
        } else {
            visibility.set_if_neq(Visibility::Hidden);
        }
    }
}

pub fn place_camera(
    s: Res<Session>,
    mut cameras: Query<(&mut Transform, &mut Projection), With<Eye>>,
) {
    let Ok((mut camera, mut lens)) = cameras.get_single_mut() else {
        return;
    };
    let mut pos = s.body.pos;
    let mut yaw = s.input.yaw;
    let mut pitch = s.input.pitch;
    if let Some(subject) = s
        .players
        .iter()
        .find(|p| Some(p.id) == s.spectate && p.hp > 0)
    {
        (pos, yaw) = s.remote_pose(subject);
        pitch = subject.pitch;
    } else if let Some(me) = s.me().filter(|me| me.hp == 0) {
        yaw = me.yaw;
        pitch = me.pitch;
    } else {
        // Clip the visual extrapolation to one unobstructed simulation tick.
        let offset = s.body.vel * s.accumulator;
        if let Some(dir) = offset.try_normalize() {
            pos += dir
                * s.map
                    .ray_distance(pos + Vec3::Y * EYE_HEIGHT, dir, offset.length());
        }
    }
    camera.translation = pos + Vec3::Y * EYE_HEIGHT;
    camera.rotation = Quat::from_euler(EulerRot::YXZ, -yaw, pitch, 0.);
    if let Projection::Perspective(lens) = &mut *lens {
        lens.fov = if s.scoped() { 27f32 } else { 73f32 }.to_radians();
    }
}

#[derive(Default)]
pub struct WeaponMotion {
    angles: Option<Vec2>,
    sway: Vec2,
    stride: f32,
    subject: Option<(u16, u32)>,
}

pub fn present_weapon(
    s: Res<Session>,
    assets: Res<SceneAssets>,
    time: Res<Time>,
    mut motion: Local<WeaponMotion>,
    mut roots: Query<(&mut Transform, &mut Visibility), With<ViewModel>>,
    mut parts: Query<(&HeldPart, &mut Transform, &mut Visibility, &mut Mesh3d), Without<ViewModel>>,
) {
    let Ok((mut root, mut visible)) = roots.get_single_mut() else {
        return;
    };
    let id = s.spectate.or(s.id);
    let Some(subject) = s
        .players
        .iter()
        .find(|p| Some(p.id) == id && p.hp > 0 && p.armed && s.mode != Mode::Surf)
    else {
        visible.set_if_neq(Visibility::Hidden);
        motion.subject = None;
        return;
    };
    visible.set_if_neq(if s.scoped() {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    });
    let rifle = s.view_rifle();
    let weapon = rifle.weapon;
    let art = &assets.weapons[weapon.index()];
    let dt = time.delta_secs().min(0.05);
    let own = s.id == Some(subject.id);
    let angles = if own {
        Vec2::new(s.input.yaw, s.input.pitch)
    } else {
        Vec2::new(subject.yaw, subject.pitch)
    };
    if motion.subject != Some((subject.id, subject.life)) {
        *motion = WeaponMotion {
            subject: Some((subject.id, subject.life)),
            ..default()
        };
    }
    let previous = motion.angles.replace(angles).unwrap_or(angles);
    let yaw_delta = (angles.x - previous.x).sin();
    let target = (Vec2::new(yaw_delta, angles.y - previous.y) / dt.max(0.001))
        .clamp(Vec2::splat(-4.), Vec2::splat(4.));
    motion.sway = motion.sway.lerp(target, 1. - (-dt * 12.).exp());
    let body = if own { s.body } else { subject.body };
    let speed = if body.grounded {
        body.vel.xz().length()
    } else {
        0.
    };
    motion.stride += dt * speed * 2.35;
    let bob = Vec2::new(motion.stride.sin(), (motion.stride * 2.).cos()) * (speed * 0.18).min(1.);
    let shot = s
        .shot_presentation(subject.id)
        .filter(|(shot, _)| shot.weapon == weapon);
    let age = shot.map_or(f32::INFINITY, |(_, at)| at.elapsed().as_secs_f32());
    let kick = (1. - age / 0.16).clamp(0., 1.).powi(2);
    let strength = match weapon {
        Weapon::Knife => 0.,
        Weapon::Pistol => 0.95,
        Weapon::M4 => 0.80,
        Weapon::Ak => 1.18,
        Weapon::Sniper => 1.52,
        Weapon::Smg => 0.56,
    };
    let slash = if weapon == Weapon::Knife && age < 0.25 {
        (age / 0.25 * std::f32::consts::PI).sin()
    } else {
        0.
    };
    let reload = s
        .reload_fraction(subject.id)
        .map_or(0., |progress| (progress * std::f32::consts::PI).sin());
    let draw = f32::from(rifle.switch_left) / f32::from(weapon.spec().draw_ticks.max(1));
    let lower = reload + draw * 0.9;
    let size = if weapon == Weapon::Sniper {
        0.18
    } else {
        0.211
    };
    root.scale = Vec3::splat(size);
    root.translation = Vec3::new(
        0.091 + bob.x * 0.0027 + motion.sway.x * 0.0012 + lower * 0.012 - slash * 0.104,
        -0.080 + bob.y * 0.0019 - motion.sway.y * 0.0009 - lower * 0.038
            + slash * 0.018
            + kick * strength * 0.003,
        -0.153 + kick * strength * 0.019 + draw * 0.021,
    );
    root.rotation = Quat::from_euler(
        EulerRot::YXZ,
        0.17 - motion.sway.x * 0.013 + slash * 0.77,
        0.022 + strength * kick * 0.15 - lower * 0.32,
        -0.025 + bob.x * 0.012 - reload * 0.40 + slash * 0.62,
    );
    for (part, mut transform, mut visibility, mut mesh) in &mut parts {
        match part {
            HeldPart::Body => {
                mesh.set_if_neq(Mesh3d(art.first_person.clone()));
            }
            HeldPart::Magazine => {
                if let Some(feed) = &art.feed {
                    mesh.set_if_neq(Mesh3d(feed.clone()));
                    transform.translation =
                        Vec3::new(-0.103 * reload, -0.22 * reload, 0.06 * reload);
                    transform.rotation = Quat::from_rotation_z(-0.42 * reload);
                    visibility.set_if_neq(Visibility::Inherited);
                } else {
                    visibility.set_if_neq(Visibility::Hidden);
                }
            }
            HeldPart::Flash => {
                let lit = weapon != Weapon::Knife && age < 0.070;
                visibility.set_if_neq(if lit {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                });
                if let Some((shot, _)) = shot.filter(|_| lit) {
                    transform.translation = art.tip;
                    transform.rotation = Quat::from_rotation_z((shot.tick % 13) as f32 * 0.48);
                    transform.scale = Vec3::splat((0.88 + strength * 0.22) * (1. - age * 6.));
                }
            }
        }
    }
}
