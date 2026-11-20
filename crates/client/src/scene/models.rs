//! Original vertex-coloured low-poly props. First- and third-person equipment share silhouettes.
use bevy::{
    image::{ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::{
        mesh::PrimitiveTopology,
        render_asset::RenderAssetUsages,
        render_resource::{Extent3d, TextureDimension, TextureFormat},
    },
};
use sendit_sim::Weapon;

type Rgb = [u8; 3];
const PARKER: Rgb = [62, 68, 65];
const STEEL: Rgb = [91, 99, 99];
const DARK: Rgb = [38, 42, 41];
const GRIP: Rgb = [65, 63, 52];
const RIM: Rgb = [133, 135, 121];
const WALNUT: Rgb = [145, 89, 48];
const GLOVE: Rgb = [67, 57, 47];
const CUFF: Rgb = [97, 92, 68];
const SKIN: Rgb = [182, 136, 99];

#[derive(Clone, Default)]
struct Model {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    uvs: Vec<[f32; 2]>,
}

// Each quadrant supplies a small original surface painting. Vertex colours dye the finish.
fn finish(rgb: Rgb) -> usize {
    match rgb {
        WALNUT | [117, 72, 43] => 1,
        GLOVE | GRIP | SKIN | [129, 95, 68] | [118, 89, 66] | [174, 131, 98] => 2,
        STEEL | PARKER | DARK | RIM | [189, 198, 192] | [112, 125, 125] | [59, 99, 101] => 0,
        _ => 3,
    }
}

pub(super) fn surface_paint() -> Image {
    const TILE: usize = 128;
    const SIDE: usize = TILE * 2;
    let mut data = Vec::with_capacity(SIDE * SIDE * 4);
    let noise = |x: u32, y: u32| {
        let mut bits = x.wrapping_mul(0x9e37_79b9) ^ y.wrapping_mul(0x85eb_ca6b);
        bits = (bits ^ (bits >> 16)).wrapping_mul(0x7feb_352d);
        (bits ^ (bits >> 15)) as f32 / u32::MAX as f32
    };
    for row in 0..SIDE {
        for column in 0..SIDE {
            let (x, y) = ((column % TILE) as u32, (row % TILE) as u32);
            let grain = noise(x, y);
            let fleck = noise(x / 3, y / 3);
            let base = match column / TILE + 2 * (row / TILE) {
                0 => {
                    let scrape = y % 19 == ((x as f32 * 0.059).floor() as u32) % 19
                        && noise(x / 11, y / 6) > 0.40;
                    if scrape {
                        254.
                    } else {
                        218. + grain * 20. + noise(x / 18, y) * 11.
                    }
                }
                1 => {
                    let swirl = (y as f32 * 0.58
                        + (x as f32 * 0.052).sin() * 2.2
                        + noise(x / 38, y / 18) * 0.7)
                        .sin();
                    218. + swirl * 15. + grain * 15. + fleck * 7.
                }
                2 => 218. + fleck * 19. + grain * 12. - if grain < 0.085 { 32. } else { 0. },
                _ => {
                    let weave = if (x / 2 + y / 2) % 2 == 0 { 12. } else { -7. };
                    222. + weave + grain * 12. + fleck * 8.
                }
            };
            let value = base.clamp(0., 255.) as u8;
            data.extend_from_slice(&[value, value, value, 255]);
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: SIDE as u32,
            height: SIDE as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor::linear());
    image
}

impl Model {
    fn face(&mut self, a: Vec3, mut b: Vec3, mut c: Vec3, inside: Vec3, rgb: Rgb) {
        let mut normal = (b - a).cross(c - a).normalize_or_zero();
        if normal == Vec3::ZERO {
            return;
        }
        if normal.dot((a + b + c) / 3. - inside) < 0. {
            std::mem::swap(&mut b, &mut c);
            normal = -normal;
        }
        let color = Color::srgb_u8(rgb[0], rgb[1], rgb[2]).to_linear();
        self.positions
            .extend([a.to_array(), b.to_array(), c.to_array()]);
        self.normals.extend([normal.to_array(); 3]);
        self.colors
            .extend([[color.red, color.green, color.blue, 1.]; 3]);
        let tile = finish(rgb);
        let offset = Vec2::new((tile % 2) as f32, (tile / 2) as f32);
        let face = normal.abs();
        self.uvs.extend([a, b, c].map(|p| {
            let d = p - inside;
            let st = if face.x > face.y && face.x > face.z {
                d.zy()
            } else if face.y > face.z {
                d.zx()
            } else {
                d.xy()
            };
            let within =
                (st * 1.63 + Vec2::splat(0.5)).clamp(Vec2::splat(0.012), Vec2::splat(0.988));
            ((offset + within) * 0.5).to_array()
        }));
    }

    /// Connect two bevelled cross-sections. A wide bevel produces an octagonal silhouette.
    fn beam(
        &mut self,
        start: [f32; 3],
        end: [f32; 3],
        a: [f32; 2],
        b: [f32; 2],
        bevel: f32,
        rgb: Rgb,
    ) {
        let start = Vec3::from(start);
        let end = Vec3::from(end);
        let axis = (end - start).normalize();
        let reference = if axis.x.abs() > 0.90 {
            Vec3::Z
        } else {
            Vec3::X
        };
        let side = (reference - axis * axis.dot(reference)).normalize();
        let up = axis.cross(side);
        let k = 1. - bevel;
        let outline = [
            (-k, -1.),
            (k, -1.),
            (1., -k),
            (1., k),
            (k, 1.),
            (-k, 1.),
            (-1., k),
            (-1., -k),
        ];
        let section = |center: Vec3, size: [f32; 2]| {
            outline.map(|(x, y)| center + side * (0.5 * size[0] * x) + up * (0.5 * size[1] * y))
        };
        let lower = section(start, a);
        let upper = section(end, b);
        let middle = (start + end) * 0.5;
        for i in 0..8 {
            let j = (i + 1) % 8;
            self.face(lower[i], lower[j], upper[j], middle, rgb);
            self.face(lower[i], upper[j], upper[i], middle, rgb);
            self.face(start, lower[i], lower[j], middle, rgb);
            self.face(end, upper[j], upper[i], middle, rgb);
        }
    }

    fn block(&mut self, p: [f32; 3], s: [f32; 3], rgb: Rgb) {
        let [x, y, z] = p;
        self.beam(
            [x, y, z - s[2] * 0.5],
            [x, y, z + s[2] * 0.5],
            [s[0], s[1]],
            [s[0], s[1]],
            0.20,
            rgb,
        );
    }

    fn tube(&mut self, y: f32, back: f32, front: f32, width: f32, rgb: Rgb) {
        self.beam(
            [0., y, back],
            [0., y, front],
            [width; 2],
            [width; 2],
            0.59,
            rgb,
        );
    }

    fn append(&mut self, other: &Self) {
        self.positions.extend_from_slice(&other.positions);
        self.normals.extend_from_slice(&other.normals);
        self.colors.extend_from_slice(&other.colors);
        self.uvs.extend_from_slice(&other.uvs);
    }

    fn mesh(self) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
    }
}

pub(super) struct WeaponArt {
    pub first_person: Handle<Mesh>,
    pub feed: Option<Handle<Mesh>>,
    pub carried: Handle<Mesh>,
    pub tip: Vec3,
}

fn shoulder_piece(m: &mut Model, color: Rgb) {
    m.beam(
        [0., 0.046, 0.073],
        [0., 0.012, 0.31],
        [0.083, 0.072],
        [0.081, 0.145],
        0.24,
        color,
    );
    m.block([0., 0.012, 0.31], [0.086, 0.147, 0.024], DARK);
}

fn grip(m: &mut Model) {
    m.beam(
        [0., 0.018, 0.032],
        [0., -0.12, 0.077],
        [0.057, 0.073],
        [0.06, 0.071],
        0.35,
        GRIP,
    );
}

fn ribbed_guard(m: &mut Model, start: f32, end: f32, color: Rgb) {
    m.beam(
        [0., 0.066, start],
        [0., 0.073, end],
        [0.092, 0.098],
        [0.073, 0.074],
        0.40,
        color,
    );
    for i in 1..6 {
        let t = i as f32 / 6.;
        let z = start + (end - start) * t;
        m.block(
            [0., 0.071, z],
            [0.096 - 0.018 * t, 0.103 - 0.025 * t, 0.011],
            PARKER,
        );
    }
}

fn sight(m: &mut Model, z: f32, y: f32) {
    m.block([0., y, z], [0.019, 0.048, 0.018], DARK);
    for side in [-1., 1.] {
        m.block([side * 0.017, y + 0.012, z], [0.008, 0.04, 0.017], STEEL);
    }
}

fn knife(m: &mut Model) {
    m.beam(
        [0., 0., 0.11],
        [0., 0.036, -0.075],
        [0.065, 0.071],
        [0.061, 0.073],
        0.39,
        GRIP,
    );
    m.block([0., 0.034, -0.079], [0.074, 0.125, 0.019], PARKER);
    let outline = [
        Vec3::new(0., -0.002, -0.084),
        Vec3::new(0., 0.012, -0.23),
        Vec3::new(0., 0.087, -0.44),
        Vec3::new(0., 0.104, -0.29),
        Vec3::new(0., 0.077, -0.084),
    ];
    let inside = Vec3::new(0., 0.053, -0.19);
    for side in [-1., 1.] {
        let ridge = inside + Vec3::X * (side * 0.016);
        for i in 0..outline.len() {
            let tint = if i <= 1 {
                [189, 198, 192]
            } else {
                [112, 125, 125]
            };
            m.face(
                outline[i],
                outline[(i + 1) % outline.len()],
                ridge,
                inside,
                tint,
            );
        }
    }
}

fn pistol(m: &mut Model, feed: &mut Model) {
    m.beam(
        [0., 0.05, 0.075],
        [0., 0.05, -0.23],
        [0.069, 0.053],
        [0.063, 0.05],
        0.16,
        DARK,
    );
    m.beam(
        [0., 0.08, 0.075],
        [0., 0.08, -0.235],
        [0.076, 0.071],
        [0.069, 0.061],
        0.19,
        STEEL,
    );
    m.beam(
        [0., 0.03, 0.03],
        [0., -0.135, 0.075],
        [0.065, 0.085],
        [0.069, 0.081],
        0.3,
        GRIP,
    );
    for z in [0.016, 0.032, 0.048] {
        m.block([0., 0.081, z], [0.077, 0.046, 0.004], PARKER);
    }
    m.tube(0.079, -0.23, -0.244, 0.041, DARK);
    m.block([0., 0.118, -0.211], [0.014, 0.014, 0.019], RIM);
    m.block([0., 0.118, 0.046], [0.059, 0.014, 0.02], DARK);
    m.block([0., -0.016, -0.048], [0.023, 0.015, 0.10], PARKER);
    feed.block([0., -0.137, 0.075], [0.073, 0.02, 0.088], DARK);
}

fn carbine(m: &mut Model, feed: &mut Model) {
    shoulder_piece(m, GRIP);
    grip(m);
    m.block([0., 0.055, -0.052], [0.085, 0.098, 0.27], PARKER);
    m.tube(0.085, 0.067, -0.29, 0.074, STEEL);
    ribbed_guard(m, -0.18, -0.42, GRIP);
    m.tube(0.073, -0.40, -0.59, 0.033, STEEL);
    m.tube(0.073, -0.575, -0.635, 0.049, DARK);
    m.block([0., 0.124, 0.038], [0.043, 0.059, 0.035], PARKER);
    m.block([0., 0.125, -0.128], [0.043, 0.056, 0.037], PARKER);
    m.block([0., 0.153, -0.046], [0.051, 0.028, 0.207], STEEL);
    m.block([-0.044, 0.063, -0.029], [0.005, 0.026, 0.106], DARK);
    m.block([-0.047, 0.069, -0.047], [0.004, 0.007, 0.061], RIM);
    for z in [-0.134, 0.03] {
        m.beam(
            [-0.041, 0.036, z],
            [-0.047, 0.036, z],
            [0.012; 2],
            [0.012; 2],
            0.59,
            STEEL,
        );
    }
    for z in [0.003, 0.020, 0.037] {
        m.block([0., 0.168, z], [0.046, 0.004, 0.004], PARKER);
    }
    sight(m, -0.42, 0.13);
    feed.beam(
        [0., 0.014, -0.111],
        [0., -0.144, -0.136],
        [0.058, 0.088],
        [0.058, 0.083],
        0.2,
        STEEL,
    );
    feed.block([0., -0.145, -0.136], [0.063, 0.018, 0.086], DARK);
}

fn wood_rifle(m: &mut Model, feed: &mut Model) {
    shoulder_piece(m, WALNUT);
    grip(m);
    m.block([0., 0.052, -0.058], [0.081, 0.099, 0.30], PARKER);
    m.tube(0.096, 0.055, -0.22, 0.072, STEEL);
    m.beam(
        [0., 0.054, -0.178],
        [0., 0.064, -0.386],
        [0.095, 0.094],
        [0.077, 0.083],
        0.34,
        WALNUT,
    );
    m.tube(0.108, -0.215, -0.426, 0.043, [117, 72, 43]);
    m.block([0., 0.067, -0.375], [0.083, 0.101, 0.023], PARKER);
    m.tube(0.065, -0.37, -0.64, 0.034, STEEL);
    m.tube(0.066, -0.62, -0.67, 0.042, DARK);
    sight(m, -0.57, 0.109);
    feed.beam(
        [0., 0.015, -0.105],
        [0., -0.093, -0.136],
        [0.060, 0.094],
        [0.057, 0.093],
        0.22,
        DARK,
    );
    feed.beam(
        [0., -0.091, -0.136],
        [0., -0.173, -0.195],
        [0.059, 0.093],
        [0.058, 0.080],
        0.22,
        PARKER,
    );
}

fn sniper(m: &mut Model, feed: &mut Model) {
    shoulder_piece(m, [85, 91, 72]);
    grip(m);
    m.beam(
        [0., 0.035, 0.09],
        [0., 0.034, -0.40],
        [0.115, 0.115],
        [0.096, 0.101],
        0.30,
        [94, 102, 78],
    );
    m.tube(0.085, 0.07, -0.46, 0.086, PARKER);
    m.tube(0.075, -0.40, -0.78, 0.045, STEEL);
    m.block([0., 0.075, -0.792], [0.081, 0.074, 0.084], DARK);
    for z in [-0.155, -0.004] {
        m.block([0., 0.151, z], [0.048, 0.095, 0.027], DARK);
    }
    m.tube(0.201, 0.090, -0.218, 0.066, PARKER);
    m.beam(
        [0., 0.201, -0.189],
        [0., 0.201, -0.278],
        [0.067; 2],
        [0.111; 2],
        0.59,
        STEEL,
    );
    m.tube(0.201, 0.113, 0.077, 0.093, DARK);
    m.tube(0.201, 0.116, 0.111, 0.064, [59, 99, 101]);
    m.block([0.063, 0.079, -0.011], [0.039, 0.031, 0.051], STEEL);
    feed.block([0., -0.054, -0.136], [0.084, 0.104, 0.13], PARKER);
}

fn smg(m: &mut Model, feed: &mut Model) {
    for side in [-1., 1.] {
        m.beam(
            [side * 0.034, 0.055, 0.04],
            [side * 0.038, 0.032, 0.25],
            [0.020; 2],
            [0.020; 2],
            0.5,
            STEEL,
        );
    }
    m.block([0., 0.013, 0.255], [0.083, 0.108, 0.020], DARK);
    grip(m);
    m.tube(0.071, 0.085, -0.25, 0.088, STEEL);
    m.block([0., 0.028, -0.03], [0.08, 0.068, 0.18], PARKER);
    ribbed_guard(m, -0.14, -0.292, GRIP);
    m.tube(0.071, -0.27, -0.39, 0.044, DARK);
    sight(m, -0.277, 0.118);
    feed.beam(
        [0., 0.014, -0.093],
        [0., -0.174, -0.118],
        [0.046, 0.060],
        [0.048, 0.064],
        0.20,
        PARKER,
    );
}

fn arms(m: &mut Model, weapon: Weapon) {
    let rear = if weapon == Weapon::Knife {
        [0.019, 0.005, 0.058]
    } else {
        [0.028, -0.058, 0.060]
    };
    m.beam(
        [0.40, -0.28, 0.38],
        [0.13, -0.12, 0.16],
        [0.19, 0.185],
        [0.118, 0.127],
        0.36,
        [100, 104, 77],
    );
    m.beam(
        [0.17, -0.137, 0.19],
        [0.119, -0.11, 0.147],
        [0.129, 0.129],
        [0.113, 0.127],
        0.32,
        CUFF,
    );
    m.beam(
        [0.126, -0.111, 0.15],
        rear,
        [0.108, 0.113],
        [0.080, 0.10],
        0.36,
        GLOVE,
    );
    for i in 0..3 {
        m.block(
            [
                rear[0] - 0.028,
                rear[1] - 0.019 - i as f32 * 0.019,
                rear[2] - 0.022,
            ],
            [0.077, 0.020, 0.046],
            if i == 0 { SKIN } else { [118, 89, 66] },
        );
    }
    if weapon != Weapon::Knife {
        let z = match weapon {
            Weapon::Pistol => 0.044,
            Weapon::Sniper => -0.32,
            Weapon::Smg => -0.19,
            _ => -0.256,
        };
        let hand = [
            -0.035,
            if weapon == Weapon::Pistol {
                -0.075
            } else {
                0.014
            },
            z,
        ];
        m.beam(
            [-0.08, -0.27, 0.21],
            [-0.101, -0.11, z + 0.119],
            [0.160, 0.159],
            [0.113, 0.118],
            0.32,
            [102, 107, 81],
        );
        m.beam(
            [-0.104, -0.118, z + 0.129],
            hand,
            [0.116, 0.12],
            [0.081, 0.093],
            0.38,
            GLOVE,
        );
        let wrist = Vec3::new(-0.102, -0.108, z + 0.115);
        let knuckles = Vec3::new(-0.069, hand[1] + 0.013, z + 0.026);
        m.beam(
            wrist.to_array(),
            knuckles.to_array(),
            [0.070, 0.018],
            [0.052, 0.017],
            0.35,
            [86, 70, 52],
        );
        m.beam(
            [-0.104, -0.117, z + 0.139],
            [-0.102, -0.104, z + 0.115],
            [0.123, 0.121],
            [0.118, 0.122],
            0.33,
            CUFF,
        );
        m.beam(
            [-0.072, hand[1] - 0.002, z + 0.047],
            [-0.046, hand[1] + 0.054, z - 0.006],
            [0.038, 0.049],
            [0.035, 0.040],
            0.42,
            GLOVE,
        );
        m.beam(
            [-0.048, hand[1] + 0.051, z - 0.003],
            [-0.021, hand[1] + 0.059, z - 0.024],
            [0.033, 0.036],
            [0.029, 0.028],
            0.42,
            SKIN,
        );
        for i in 0..3 {
            m.block(
                [0.002, hand[1] - 0.018, z - 0.022 + i as f32 * 0.023],
                [0.094, 0.039, 0.021],
                [129, 95, 68],
            );
        }
    }
}

impl WeaponArt {
    pub fn new(weapon: Weapon, meshes: &mut Assets<Mesh>) -> Self {
        let mut body = Model::default();
        let mut magazine = Model::default();
        let (y, z) = match weapon {
            Weapon::Knife => {
                knife(&mut body);
                (0.087, -0.44)
            }
            Weapon::Pistol => {
                pistol(&mut body, &mut magazine);
                (0.079, -0.244)
            }
            Weapon::M4 => {
                carbine(&mut body, &mut magazine);
                (0.073, -0.635)
            }
            Weapon::Ak => {
                wood_rifle(&mut body, &mut magazine);
                (0.066, -0.67)
            }
            Weapon::Sniper => {
                sniper(&mut body, &mut magazine);
                (0.075, -0.835)
            }
            Weapon::Smg => {
                smg(&mut body, &mut magazine);
                (0.071, -0.39)
            }
        };
        let mut carried = body.clone();
        carried.append(&magazine);
        arms(&mut body, weapon);
        Self {
            first_person: meshes.add(body.mesh()),
            feed: (!magazine.positions.is_empty()).then(|| meshes.add(magazine.mesh())),
            carried: meshes.add(carried.mesh()),
            tip: Vec3::new(0., y, z),
        }
    }
}

/// Legs pivot independently about the hips while torso and head follow the network pose.
pub(super) struct Uniform {
    pub torso: Handle<Mesh>,
    pub leg: Handle<Mesh>,
}

impl Uniform {
    pub fn new(blue: bool, meshes: &mut Assets<Mesh>) -> Self {
        let cloth = if blue { [69, 103, 130] } else { [178, 96, 56] };
        let panel = if blue { [91, 131, 157] } else { [211, 124, 70] };
        let trousers = if blue { [72, 92, 101] } else { [117, 91, 67] };
        let kit = [64, 70, 60];
        let webbing = [103, 103, 78];
        let mut m = Model::default();
        m.beam(
            [0., 0.78, 0.],
            [0., 1.08, 0.],
            [0.34, 0.26],
            [0.39, 0.27],
            0.31,
            trousers,
        );
        m.beam(
            [0., 0.96, 0.],
            [0., 1.43, -0.01],
            [0.34, 0.27],
            [0.46, 0.30],
            0.26,
            cloth,
        );
        m.beam(
            [0., 1.02, -0.008],
            [0., 1.345, -0.016],
            [0.378, 0.289],
            [0.438, 0.318],
            0.23,
            kit,
        );
        m.block([0., 0.982, -0.004], [0.372, 0.069, 0.285], GLOVE);
        m.block([0., 0.983, -0.151], [0.087, 0.045, 0.016], RIM);
        for side in [-1., 1.] {
            m.beam(
                [side * 0.12, 1.21, -0.161],
                [side * 0.164, 1.41, -0.111],
                [0.056, 0.037],
                [0.064, 0.038],
                0.2,
                webbing,
            );
            m.block(
                [side * 0.091, 1.115, -0.164],
                [0.132, 0.138, 0.074],
                webbing,
            );
            m.block([side * 0.091, 1.153, -0.197], [0.134, 0.045, 0.019], kit);
            m.block([side * 0.18, 0.957, 0.011], [0.087, 0.091, 0.118], kit);
        }
        m.block([0., 1.204, 0.147], [0.270, 0.260, 0.080], kit);
        m.beam(
            [0., 1.41, -0.014],
            [0., 1.49, -0.015],
            [0.14, 0.14],
            [0.15, 0.15],
            0.49,
            SKIN,
        );
        m.beam(
            [0., 1.447, -0.029],
            [0., 1.621, -0.02],
            [0.173, 0.175],
            [0.217, 0.214],
            0.40,
            [174, 131, 98],
        );
        m.beam(
            [0., 1.55, -0.027],
            [0., 1.62, -0.027],
            [0.20, 0.198],
            [0.222, 0.221],
            0.42,
            GRIP,
        );
        m.block([0., 1.573, -0.125], [0.184, 0.052, 0.038], [83, 103, 101]);
        m.beam(
            [0., 1.617, -0.012],
            [0., 1.684, -0.008],
            [0.267, 0.284],
            [0.262, 0.276],
            0.46,
            kit,
        );
        m.beam(
            [0., 1.68, -0.008],
            [0., 1.761, 0.],
            [0.259, 0.270],
            [0.167, 0.173],
            0.48,
            webbing,
        );
        for (shoulder, elbow, hand) in [
            (
                [0.223, 1.365, -0.012],
                [0.277, 1.106, 0.008],
                [0.128, 1.22, -0.192],
            ),
            (
                [-0.223, 1.365, -0.012],
                [-0.256, 1.121, -0.105],
                [0.050, 1.22, -0.409],
            ),
        ] {
            m.beam(shoulder, elbow, [0.173, 0.172], [0.132, 0.145], 0.38, panel);
            m.beam(elbow, hand, [0.129, 0.140], [0.094, 0.095], 0.38, cloth);
            m.block(hand, [0.108, 0.09, 0.113], GLOVE);
        }
        let mut leg = Model::default();
        leg.beam(
            [0., 0., 0.],
            [0., -0.385, -0.025],
            [0.164, 0.195],
            [0.138, 0.141],
            0.31,
            trousers,
        );
        leg.beam(
            [0., -0.38, -0.025],
            [0., -0.703, 0.015],
            [0.145, 0.145],
            [0.111, 0.12],
            0.31,
            trousers,
        );
        leg.block([0., -0.37, -0.084], [0.140, 0.137, 0.067], kit);
        leg.beam(
            [0., -0.624, 0.012],
            [0., -0.778, 0.012],
            [0.126, 0.139],
            [0.146, 0.172],
            0.29,
            DARK,
        );
        leg.block([0., -0.776, -0.052], [0.151, 0.094, 0.266], GLOVE);
        leg.block([0., -0.815, -0.051], [0.156, 0.029, 0.275], DARK);
        Self {
            torso: meshes.add(m.mesh()),
            leg: meshes.add(leg.mesh()),
        }
    }
}

pub(super) fn flash_mesh() -> Mesh {
    let mut m = Model::default();
    let inside = Vec3::new(0., 0., -0.08);
    for i in 0..9 {
        let a = i as f32 * std::f32::consts::TAU / 9.;
        let b = a + 0.40;
        let c = a + 0.68;
        let arm = 0.102 + 0.038 * (i % 3) as f32;
        let base = Vec3::new(a.cos() * 0.024, a.sin() * 0.024, 0.);
        let tip = Vec3::new(b.cos() * arm, b.sin() * arm, -0.11 - 0.038 * (i % 4) as f32);
        let edge = Vec3::new(c.cos() * 0.049, c.sin() * 0.049, -0.059);
        m.face(base, tip, edge, inside, [255, 182, 63]);
        m.face(base, edge, inside, inside + Vec3::Y * 0.01, [255, 248, 175]);
    }
    m.beam(
        [0., 0., 0.],
        [0., 0., -0.18],
        [0.047; 2],
        [0.011; 2],
        0.56,
        [255, 250, 210],
    );
    m.mesh()
}
