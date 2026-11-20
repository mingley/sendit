//! Build render meshes directly from the collision solids. One static draw group per material.
use crate::game::Session;
use bevy::{
    core_pipeline::tonemapping::Tonemapping,
    prelude::*,
    render::{mesh::PrimitiveTopology, render_asset::RenderAssetUsages},
};
use sendit_sim::{map::Material, motion::EYE_HEIGHT, BombView};
use std::collections::HashMap;

const PALETTE: [[u8; 3]; 9] = [
    [107, 124, 124],
    [210, 207, 190],
    [58, 79, 82],
    [233, 156, 64],
    [203, 69, 46],
    [135, 159, 156],
    [157, 162, 151],
    [211, 109, 59],
    [83, 150, 174],
];
fn slot(material: Material) -> usize {
    match material {
        Material::Floor => 0,
        Material::Wall => 1,
        Material::Cover => 2,
        Material::Trim => 3,
        Material::Target => 4,
        Material::Ramp => 5,
        Material::Step => 6,
        Material::SiteA => 7,
        Material::SiteB => 8,
    }
}

#[derive(Default, Clone)]
struct Batch {
    vertices: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
}
impl Batch {
    fn face(&mut self, a: Vec3, b: Vec3, c: Vec3) {
        let normal = (b - a).cross(c - a);
        if normal.length_squared() < 1e-15 {
            return;
        }
        self.vertices
            .extend([a.to_array(), b.to_array(), c.to_array()]);
        self.normals.extend([normal.normalize().to_array(); 3]);
    }
    fn prism(&mut self, min: Vec3, max: Vec3, height: impl Fn(f32, f32) -> f32) {
        let bottom = [
            Vec3::new(min.x, min.y, min.z),
            Vec3::new(min.x, min.y, max.z),
            Vec3::new(max.x, min.y, max.z),
            Vec3::new(max.x, min.y, min.z),
        ];
        let top = bottom.map(|p| p.with_y(height(p.x, p.z)));
        self.face(top[0], top[1], top[2]);
        self.face(top[0], top[2], top[3]);
        self.face(bottom[0], bottom[2], bottom[1]);
        self.face(bottom[0], bottom[3], bottom[2]);
        for i in 0..4 {
            let j = (i + 1) % 4;
            self.face(bottom[i], bottom[j], top[j]);
            self.face(bottom[i], top[j], top[i]);
        }
    }
    fn block(&mut self, center: [f32; 3], size: [f32; 3]) {
        let c = Vec3::from(center);
        let half = Vec3::from(size) * 0.5;
        self.prism(c - half, c + half, |_, _| c.y + half.y);
    }
    fn floor_stripe(&mut self, x: f32, z: f32, width: f32, length: f32) {
        let corner = |dx: f32, dz: f32| Vec3::new(x + dx * width, 0.008, z + dz * length);
        let [a, b, c, d] = [
            corner(-0.5, -0.5),
            corner(-0.5, 0.5),
            corner(0.5, 0.5),
            corner(0.5, -0.5),
        ];
        self.face(a, b, c);
        self.face(a, c, d);
    }
    // Painted sidelines and distance marks give strafing players a reference on the training court.
    fn aim_markings(&mut self) {
        for side in [-1., 1.] {
            self.floor_stripe(side * 12.5, 0., 0.13, 23.);
            self.floor_stripe(0., side * 11.5, 25.13, 0.13);
            for z in [-9., -6., -3., 0., 3., 6., 9.] {
                self.floor_stripe(side * 12.02, z, 0.82, 0.13);
            }
            for x in [3.6, 5.5, 7.4, 9.3, 11.2] {
                self.floor_stripe(side * x, 0., 0.9, 0.15);
            }
        }
    }
    fn mesh(self) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.vertices)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
    }
}

#[derive(Component)]
pub struct Geometry;
#[derive(Component)]
pub struct Eye;
#[derive(Component)]
pub struct ViewModel;
#[derive(Component)]
pub struct Remote;
#[derive(Component)]
pub struct WornGear(u16);
#[derive(Component)]
pub struct Objective;
#[derive(Resource)]
pub struct SceneAssets {
    palette: Vec<Handle<StandardMaterial>>,
    uniform: [Handle<StandardMaterial>; 2],
    dark: Handle<StandardMaterial>,
    suit: Handle<Mesh>,
    gear: Handle<Mesh>,
    unarmed_gear: Handle<Mesh>,
}

fn material(materials: &mut Assets<StandardMaterial>, rgb: [u8; 3]) -> Handle<StandardMaterial> {
    materials.add(StandardMaterial {
        base_color: Color::srgb_u8(rgb[0], rgb[1], rgb[2]),
        perceptual_roughness: 0.92,
        reflectance: 0.12,
        ..default()
    })
}

pub fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    let palette = PALETTE.map(|rgb| material(&mut mats, rgb)).to_vec();
    // Shoulder colors remain readable against the pale concrete and against each other.
    let uniform = [[183, 84, 44], [41, 102, 150]].map(|rgb| {
        let dye = Color::srgb_u8(rgb[0], rgb[1], rgb[2]);
        // A small, fixed color contribution keeps team silhouettes readable in unlit corners.
        mats.add(StandardMaterial {
            base_color: dye,
            emissive: dye.to_linear() * 0.36,
            perceptual_roughness: 0.92,
            reflectance: 0.12,
            ..default()
        })
    });
    let dark = material(&mut mats, [34, 46, 49]);
    let mut suit = Batch::default();
    let mut gear = Batch::default();
    suit.block([0., 1.07, 0.], [0.44, 0.56, 0.30]);
    suit.block([0., 1.57, 0.], [0.30, 0.31, 0.31]);
    for side in [-1., 1.] {
        suit.block([side * 0.25, 1.18, -0.045], [0.145, 0.26, 0.20]);
        gear.block([side * 0.24, 1.05, -0.14], [0.14, 0.13, 0.33]);
        suit.block([side * 0.12, 0.47, 0.], [0.19, 0.67, 0.22]);
        gear.block([side * 0.12, 0.095, -0.05], [0.20, 0.19, 0.32]);
    }
    gear.block([0., 0.79, 0.], [0.42, 0.13, 0.32]);
    gear.block([0., 1.55, -0.161], [0.26, 0.072, 0.025]);
    let unarmed_gear = meshes.add(gear.clone().mesh());
    gear.block([0.145, 1.16, -0.34], [0.075, 0.125, 0.55]);
    gear.block([0.145, 1.16, -0.66], [0.035, 0.035, 0.16]);

    // The viewmodel fits between the near plane and the player's collision radius;
    // the crosshair remains clear even against a wall.
    let mut receiver = Batch::default();
    let mut stock = Batch::default();
    let mut brass = Batch::default();
    receiver.block([0.092, -0.076, -0.162], [0.039, 0.042, 0.108]);
    receiver.block([0.086, -0.060, -0.220], [0.030, 0.028, 0.057]);
    receiver.block([0.084, -0.060, -0.265], [0.014, 0.014, 0.044]);
    receiver.block([0.086, -0.041, -0.240], [0.004, 0.017, 0.007]);
    stock.block([0.107, -0.092, -0.092], [0.050, 0.051, 0.060]);
    stock.block([0.093, -0.104, -0.172], [0.022, 0.044, 0.038]);
    for i in 0..4 {
        receiver.block(
            [0.087, -0.044, -0.190 - i as f32 * 0.011],
            [0.029, 0.005, 0.004],
        );
    }
    brass.block([0.112, -0.071, -0.157], [0.002, 0.010, 0.025]);
    let gun = [
        (receiver.mesh(), dark.clone()),
        (stock.mesh(), palette[2].clone()),
        (brass.mesh(), palette[3].clone()),
    ];
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
                .spawn((ViewModel, Transform::default(), Visibility::default()))
                .with_children(|gun_root| {
                    for (mesh, mat) in gun {
                        gun_root.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(mat)));
                    }
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
    // The C4 carrier is shown on the HUD. A dropped or planted pack also has a world-space silhouette.
    let mut pack = Batch::default();
    pack.block([0., 0.10, 0.], [0.36, 0.20, 0.28]);
    commands.spawn((
        Mesh3d(meshes.add(pack.mesh())),
        MeshMaterial3d(palette[3].clone()),
        Objective,
        Visibility::Hidden,
    ));
    commands.insert_resource(SceneAssets {
        palette,
        uniform,
        dark,
        suit: meshes.add(suit.mesh()),
        gear: meshes.add(gear.mesh()),
        unarmed_gear,
    });
}

pub fn sync_map(
    mut commands: Commands,
    s: Res<Session>,
    assets: Res<SceneAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    old: Query<Entity, With<Geometry>>,
    mut generation: Local<u64>,
) {
    if *generation == s.map_revision {
        return;
    }
    for entity in &old {
        commands.entity(entity).despawn();
    }
    let mut parts: [Batch; 9] = Default::default();
    for b in &s.map.boxes {
        parts[slot(b.material)].prism(b.min, b.max, |_, _| b.max.y);
    }
    for r in &s.map.ramps {
        parts[slot(r.material)].prism(r.min, r.max, |x, z| r.height_at(x, z));
    }
    if s.map.name == "aim" {
        parts[slot(Material::Step)].aim_markings();
    }
    for (index, batch) in parts.into_iter().enumerate() {
        if !batch.vertices.is_empty() {
            commands.spawn((
                Mesh3d(meshes.add(batch.mesh())),
                MeshMaterial3d(assets.palette[index].clone()),
                Geometry,
            ));
        }
    }
    *generation = s.map_revision;
}

pub fn sync_players(
    mut commands: Commands,
    s: Res<Session>,
    assets: Res<SceneAssets>,
    mut positions: Query<
        (
            &mut Transform,
            &mut Visibility,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        With<Remote>,
    >,
    mut equipment: Query<(&WornGear, &mut Mesh3d)>,
    mut entities: Local<HashMap<u16, Entity>>,
    mut objectives: Query<(&mut Transform, &mut Visibility), (With<Objective>, Without<Remote>)>,
) {
    entities.retain(|id, entity| {
        let keep = s.players.iter().any(|p| p.id == *id && Some(p.id) != s.id);
        if !keep {
            commands.entity(*entity).despawn_recursive();
        }
        keep
    });
    for (gear, mut mesh) in &mut equipment {
        let armed =
            s.mode != sendit_sim::Mode::Surf && s.players.iter().any(|p| p.id == gear.0 && p.armed);
        let source = if armed {
            &assets.gear
        } else {
            &assets.unarmed_gear
        };
        if mesh.0 != *source {
            mesh.0 = source.clone();
        }
    }
    for p in s.players.iter().filter(|v| Some(v.id) != s.id) {
        let (pos, yaw) = s.remote_pose(p);
        let transform = Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(-yaw));
        let team_spectator_waiting = s.mode == sendit_sim::Mode::Competitive
            && s.spectate.is_none()
            && s.me().is_some_and(|me| me.hp == 0);
        let visibility = if p.hp == 0 || s.spectate == Some(p.id) || team_spectator_waiting {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        // Every other player is a rival in free-for-all, even across the two spawn groups.
        let blue = s.mode != sendit_sim::Mode::Deathmatch && p.team != 0;
        let paint = assets.uniform[usize::from(blue)].clone();
        if let Some(entity) = entities.get(&p.id) {
            if let Ok((mut t, mut v, mut m)) = positions.get_mut(*entity) {
                *t = transform;
                v.set_if_neq(visibility);
                if m.0 != paint {
                    m.0 = paint;
                }
            }
        } else {
            let entity = commands
                .spawn((
                    Mesh3d(assets.suit.clone()),
                    MeshMaterial3d(paint),
                    transform,
                    visibility,
                    Remote,
                ))
                .with_children(|root| {
                    root.spawn((
                        WornGear(p.id),
                        Mesh3d(if s.mode != sendit_sim::Mode::Surf && p.armed {
                            assets.gear.clone()
                        } else {
                            assets.unarmed_gear.clone()
                        }),
                        MeshMaterial3d(assets.dark.clone()),
                    ));
                })
                .id();
            entities.insert(p.id, entity);
        }
    }
    if let Ok((mut transform, mut visibility)) = objectives.get_single_mut() {
        let position = s.round.as_ref().and_then(|r| match r.bomb {
            BombView::Dropped(pos) | BombView::Planted(_, pos) => Some(pos),
            _ => None,
        });
        if let Some(pos) = position {
            transform.translation = pos;
            *visibility = Visibility::Inherited;
        } else {
            *visibility = Visibility::Hidden;
        }
    }
}

pub fn place_camera(
    s: Res<Session>,
    mut cameras: Query<&mut Transform, With<Eye>>,
    mut guns: Query<&mut Visibility, With<ViewModel>>,
) {
    let Ok(mut camera) = cameras.get_single_mut() else {
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
        // Extrapolate at most one tick, then clip the visual offset against the map.
        // The fixed-tick body used by prediction and hit registration is never shifted.
        let offset = s.body.vel * s.accumulator;
        if offset.length_squared() > 0. {
            let distance = s.map.ray_distance(
                pos + Vec3::Y * EYE_HEIGHT,
                offset.normalize(),
                offset.length(),
            );
            pos += offset.normalize() * distance;
        }
    }
    camera.translation = pos + Vec3::Y * EYE_HEIGHT;
    camera.rotation = Quat::from_euler(EulerRot::YXZ, -yaw, pitch, 0.);
    if let Ok(mut visibility) = guns.get_single_mut() {
        visibility.set_if_neq(
            if s.mode != sendit_sim::Mode::Surf && s.me().is_some_and(|p| p.armed && p.hp > 0) {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            },
        );
    }
}
