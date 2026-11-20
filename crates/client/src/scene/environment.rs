//! The solid faces use the simulation mesh exactly. Yard architecture beyond the boundary,
//! flush wall cladding and floor paint are render-only. All art is embedded for standalone play.
use crate::game::Session;
use bevy::{
    image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::{
        mesh::{Indices, PrimitiveTopology},
        render_asset::RenderAssetUsages,
        render_resource::{Extent3d, TextureDimension, TextureFormat},
    },
};
use sendit_sim::map::{Arena, Material, Solid};
use std::f32::consts::{FRAC_PI_2, PI, TAU};

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
enum Surface {
    Stone,
    Brick,
    Pavers,
    Slate,
    Cargo,
    LowFreight,
    Deck,
    Iron,
    Shutter,
    Window,
    Hazard,
    Drain,
    SiteA,
    SiteB,
    Target,
    OchreMark,
    BlueMark,
    PaleStone,
    Soot,
    Sky,
}
use Surface::*;

macro_rules! tile {
    ($name:literal) => {
        include_bytes!(concat!("../../assets/environment/", $name, ".ppm")).as_slice()
    };
}

#[derive(Resource)]
pub(crate) struct EnvironmentMaterials(Vec<Handle<StandardMaterial>>);
#[derive(Component)]
pub(crate) struct WorldGeometry;

/// These original P6 paintings have a fixed, editable RGB layout. Supply the complete mip
/// chain so distant boards, bricks and grilles stay stable under motion and grazing angles.
fn tile_image(source: &[u8]) -> Image {
    const SIZE: usize = 256;
    let rgb = source
        .strip_prefix(b"P6\n256 256\n255\n")
        .expect("256-pixel P6 tile");
    assert_eq!(rgb.len(), SIZE * SIZE * 3);
    let pixels = rgb
        .chunks_exact(3)
        .flat_map(|p| [p[0], p[1], p[2], 255])
        .collect();
    let mut image = Image::new(
        Extent3d {
            width: SIZE as u32,
            height: SIZE as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    let mut width = SIZE;
    let mut previous = 0;
    while width > 1 {
        let next = image.data.len();
        for y in 0..width / 2 {
            for x in 0..width / 2 {
                let a = previous + 4 * (2 * y * width + 2 * x);
                let b = a + width * 4;
                for c in 0..3 {
                    let sum = image.data[a + c] as u16
                        + image.data[a + 4 + c] as u16
                        + image.data[b + c] as u16
                        + image.data[b + 4 + c] as u16;
                    image.data.push(((sum + 2) / 4) as u8);
                }
                image.data.push(255);
            }
        }
        previous = next;
        width /= 2;
        image.texture_descriptor.mip_level_count += 1;
    }
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        anisotropy_clamp: 4,
        ..ImageSamplerDescriptor::linear()
    });
    image
}

pub(crate) fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    let sources = [
        tile!("limestone"),
        tile!("kiln-brick"),
        tile!("paving"),
        tile!("slate"),
        tile!("shipping-frame"),
        tile!("deckboards"),
        tile!("iron"),
        tile!("roller"),
        tile!("glazing"),
        tile!("safety"),
        tile!("grate"),
        tile!("harbor-sky"),
        tile!("low-freight"),
        tile!("site-ochre"),
        tile!("site-blue"),
        tile!("red-oxide"),
    ];
    let textures = sources.map(|source| images.add(tile_image(source)));
    // StandardMaterial tint is in sRGB. The original surface paintings supply the base hues.
    let plain = [255, 255, 255];
    let finishes = [
        (0, plain),
        (1, plain),
        (2, [236, 236, 232]),
        (3, plain),
        (4, plain),
        (12, plain),
        (5, plain),
        (6, plain),
        (7, plain),
        (8, plain),
        (9, plain),
        (10, plain),
        (13, plain),
        (14, plain),
        (15, plain),
        (9, [255, 192, 115]),
        (6, [128, 214, 248]),
        (0, [255, 249, 224]),
        (6, [108, 127, 130]),
        (11, plain),
    ];
    let handles = finishes
        .iter()
        .enumerate()
        .map(|(i, (tile, rgb))| {
            mats.add(StandardMaterial {
                base_color: Color::srgb_u8(rgb[0], rgb[1], rgb[2]),
                base_color_texture: Some(textures[*tile].clone()),
                perceptual_roughness: if i == Window as usize { 0.68 } else { 0.95 },
                reflectance: 0.10,
                unlit: i == Sky as usize,
                ..default()
            })
        })
        .collect();
    commands.insert_resource(EnvironmentMaterials(handles));
}

#[derive(Default)]
struct Batch {
    p: Vec<[f32; 3]>,
    n: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    color: Vec<[f32; 4]>,
    indices: Vec<u32>,
}
impl Batch {
    fn vertex(&mut self, p: Vec3, n: Vec3, uv: Vec2, shade: f32) {
        self.p.push(p.to_array());
        self.n.push(n.to_array());
        self.uv.push(uv.to_array());
        self.color.push([shade, shade, shade, 1.]);
    }
    fn quad(&mut self, p: [Vec3; 4], uv: [Vec2; 4], shade: [f32; 4]) {
        let n = (p[1] - p[0]).cross(p[2] - p[0]) + (p[2] - p[0]).cross(p[3] - p[0]);
        if n.length_squared() < 1e-12 {
            return;
        }
        let n = n.normalize();
        let i = self.p.len() as u32;
        for k in 0..4 {
            self.vertex(p[k], n, uv[k], shade[k]);
        }
        self.indices.extend([i, i + 1, i + 2, i, i + 2, i + 3]);
    }
    fn mesh(self) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.p)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.n)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uv)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.color)
        .with_inserted_indices(Indices::U32(self.indices))
    }
}

const UV_FACE: [Vec2; 4] = [
    Vec2::new(0., 1.),
    Vec2::new(1., 1.),
    Vec2::new(1., 0.),
    Vec2::new(0., 0.),
];
const COUNT: usize = Sky as usize + 1;
#[derive(Default)]
struct Yard {
    batches: [Batch; COUNT],
}

fn projected(p: Vec3, n: Vec3, surface: Surface) -> Vec2 {
    let scale = match surface {
        Stone | PaleStone => 3.4,
        Brick => 2.7,
        Pavers | SiteA | SiteB => 3.1,
        Slate => 2.8,
        Iron | Soot => 2.1,
        Shutter => 2.55,
        Hazard | Target | OchreMark | BlueMark => 1.1,
        Deck | Cargo | LowFreight => 2.2,
        Drain => 0.8,
        _ => 1.,
    };
    if n.y.abs() >= n.x.abs() && n.y.abs() >= n.z.abs() {
        Vec2::new(p.x, p.z) / scale
    } else if n.z.abs() >= n.x.abs() {
        Vec2::new(p.x * n.z.signum(), -p.y) / scale
    } else {
        Vec2::new(-p.z * n.x.signum(), -p.y) / scale
    }
}
fn corners(min: Vec3, max: Vec3) -> [Vec3; 4] {
    [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(max.x, min.y, min.z),
    ]
}
// Warm light bouncing off the courtyard paving opens shadow-side materials when viewed
// from the opposite spawn. This is a per-vertex bake; it adds no light or shadow pass.
fn bounce(normal: Vec3) -> f32 {
    let facing = normal.dot(Vec3::new(-4., 9., 5.).normalize());
    1. + 0.89 * (0.1 - facing * 2.5).clamp(0., 1.) * (1. - normal.y.abs())
}
fn wall_shade(p: Vec3) -> f32 {
    0.76 + 0.24 * (p.y / 2.).clamp(0., 1.)
}
impl Yard {
    fn face(&mut self, surface: Surface, p: [Vec3; 4], shade: [f32; 4]) {
        let n = ((p[1] - p[0]).cross(p[2] - p[0]) + (p[2] - p[0]).cross(p[3] - p[0]))
            .normalize_or_zero();
        let uv = if matches!(surface, Cargo | LowFreight | Window) {
            let count = if matches!(surface, Cargo | LowFreight) {
                ((p[1] - p[0]).length() / 2.35).round().max(1.)
            } else {
                1.
            };
            UV_FACE.map(|uv| Vec2::new(uv.x * count, uv.y))
        } else if surface == Deck && n.y.abs() < 0.1 {
            // The low wooden lid forms one horizontal beam across the crate corners.
            UV_FACE.map(|uv| Vec2::new(0.18 + 0.073 * uv.y, uv.x * (p[1] - p[0]).length() / 1.9))
        } else {
            p.map(|v| projected(v, n, surface))
        };
        self.batches[surface as usize].quad(p, uv, shade.map(|v| v * bounce(n)));
    }
    fn shell(
        &mut self,
        side: Surface,
        top: Surface,
        base: [Vec3; 4],
        cap: [Vec3; 4],
        draw_cap: bool,
    ) {
        if draw_cap {
            self.face(top, cap, [1.; 4]);
        }
        self.face(side, [base[3], base[2], base[1], base[0]], [0.72; 4]);
        for i in 0..4 {
            let j = (i + 1) % 4;
            let p = [base[i], base[j], cap[j], cap[i]];
            self.face(side, p, p.map(wall_shade));
        }
    }
    fn block(&mut self, side: Surface, top: Surface, min: [f32; 3], max: [f32; 3]) {
        let base = corners(min.into(), max.into());
        self.shell(side, top, base, base.map(|v| v.with_y(max[1])), true);
    }
    fn rectangle(&mut self, s: Surface, points: [Vec3; 4], repeats: Vec2) {
        let n = (points[1] - points[0])
            .cross(points[3] - points[0])
            .normalize_or_zero();
        self.batches[s as usize].quad(points, UV_FACE.map(|uv| uv * repeats), [bounce(n); 4]);
    }
    fn beam(&mut self, s: Surface, a: Vec3, b: Vec3, half: f32) {
        let direction = (b - a).normalize();
        let reference = if direction.y.abs() > 0.90 {
            Vec3::X
        } else {
            Vec3::Y
        };
        let u = direction.cross(reference).normalize() * half;
        let v = direction.cross(u).normalize() * half;
        let section = |p| [p - u - v, p + u - v, p + u + v, p - u + v];
        self.shell(s, s, section(a), section(b), true);
    }
    fn floor_grid(&mut self, s: Surface, solid: Solid, arena: &Arena) {
        let min = solid.min;
        let max = solid.max;
        let nx = ((max.x - min.x) / 0.43).ceil().clamp(1., 90.) as u32;
        let nz = ((max.z - min.z) / 0.43).ceil().clamp(1., 90.) as u32;
        let batch = &mut self.batches[s as usize];
        let start = batch.p.len() as u32;
        let light = Vec3::new(-4., 9., 5.).normalize();
        for z in 0..=nz {
            for x in 0..=nx {
                let p = Vec3::new(
                    min.x + (max.x - min.x) * (x as f32 / nx as f32),
                    max.y,
                    min.z + (max.z - min.z) * (z as f32 / nz as f32),
                );
                let mut near = 1.3_f32;
                for b in &arena.boxes {
                    if b.max.y < p.y + 0.30 || b.min.y > p.y + 0.24 {
                        continue;
                    }
                    let dx = (b.min.x - p.x).max(p.x - b.max.x).max(0.);
                    let dz = (b.min.z - p.z).max(p.z - b.max.z).max(0.);
                    near = near.min(dx.max(dz));
                }
                let occlusion = 0.72 + 0.28 * (near / 1.3);
                let lit = if arena.ray_distance(p + Vec3::Y * 0.045, light, 22.) < 21.99 {
                    0.63
                } else {
                    1.
                };
                let wear = if arena.name == "aim" {
                    let path = ((p.x.abs() - 4.35).abs() - 0.81).abs();
                    let track =
                        (1. - path / 0.34).clamp(0., 1.) * ((p.z.abs() - 4.0) / 6.).clamp(0., 1.);
                    0.97 - 0.15 * track + 0.035 * (0.67 * p.z + 0.37 * p.x).sin()
                } else {
                    1.
                };
                batch.vertex(p, Vec3::Y, projected(p, Vec3::Y, s), lit * occlusion * wear);
            }
        }
        for z in 0..nz {
            for x in 0..nx {
                let i = start + z * (nx + 1) + x;
                batch
                    .indices
                    .extend([i, i + nx + 1, i + nx + 2, i, i + nx + 2, i + 1]);
            }
        }
    }
    fn collision_mesh(&mut self, arena: &Arena) {
        for b in &arena.boxes {
            let size = b.max - b.min;
            let (side, top) = match b.material {
                Material::Floor => (Slate, Pavers),
                Material::Wall => {
                    let s = if arena.name == "aim" && b.min.z > 12. {
                        Brick
                    } else {
                        Stone
                    };
                    (s, PaleStone)
                }
                Material::Cover if arena.name == "aim" && b.min.y > 1.0 && size.z < 0.28 => {
                    (Soot, Iron)
                }
                Material::Cover if size.y < 1.4 && size.x.min(size.z) > 0.65 => (LowFreight, Deck),
                Material::Cover
                    if size.x.min(size.z) > 0.65 && size.y < 2.7 * size.x.min(size.z) =>
                {
                    (Cargo, Deck)
                }
                Material::Cover => (Iron, Iron),
                Material::Trim if arena.name == "aim" && size.y < 0.22 => (Deck, Deck),
                Material::Trim => (Hazard, Hazard),
                Material::Ramp => (Iron, Iron),
                Material::Step => (Slate, Slate),
                Material::Target => (Target, Target),
                Material::SiteA if b.min.y >= 0. => (OchreMark, OchreMark),
                Material::SiteB if b.min.y >= 0. => (BlueMark, BlueMark),
                Material::SiteA => (SiteA, SiteA),
                Material::SiteB => (SiteB, SiteB),
            };
            let base = corners(b.min, b.max);
            // Only the few authored ground slabs need baked vertex lighting; arbitrary maps
            // retain constant construction costs per brush.
            let baked = matches!(arena.name.as_str(), "aim" | "drill" | "relay" | "surf")
                && arena.boxes.len() < 130
                && b.max.y.abs() < 0.04
                && matches!(
                    b.material,
                    Material::Floor | Material::SiteA | Material::SiteB
                );
            self.shell(side, top, base, base.map(|v| v.with_y(b.max.y)), !baked);
            if baked {
                self.floor_grid(top, *b, arena);
            }
        }
        for r in &arena.ramps {
            let base = corners(r.min, r.max);
            let surface = match r.material {
                Material::SiteA => SiteA,
                Material::SiteB => SiteB,
                Material::Trim => Hazard,
                _ => Iron,
            };
            self.shell(
                Slate,
                surface,
                base,
                base.map(|p| p.with_y(r.height_at(p.x, p.z))),
                true,
            );
        }
    }
}

impl Yard {
    // A panel on a Z-facing facade. The chosen winding always faces the courtyard.
    fn panel(&mut self, s: Surface, z: f32, facing: f32, x: [f32; 2], y: [f32; 2], repeats: Vec2) {
        let [a, b] = if facing > 0. { x } else { [x[1], x[0]] };
        self.rectangle(
            s,
            [
                Vec3::new(a, y[0], z),
                Vec3::new(b, y[0], z),
                Vec3::new(b, y[1], z),
                Vec3::new(a, y[1], z),
            ],
            repeats,
        );
    }
    fn lintel_window(&mut self, x: f32, z: f32, facing: f32, bottom: f32, width: f32, height: f32) {
        let edge = [x - width / 2., x + width / 2.];
        self.panel(
            Soot,
            z + facing * 0.006,
            facing,
            [edge[0] - 0.09, edge[1] + 0.09],
            [bottom - 0.07, bottom + height + 0.08],
            Vec2::ONE,
        );
        self.panel(
            Window,
            z + facing * 0.009,
            facing,
            edge,
            [bottom, bottom + height],
            Vec2::new((width / 2.2).round().max(1.), 1.),
        );
        self.panel(
            PaleStone,
            z + facing * 0.014,
            facing,
            [edge[0] - 0.15, edge[1] + 0.15],
            [bottom - 0.14, bottom + 0.01],
            Vec2::new(1.1, 0.16),
        );
    }

    // The closed shutters and stone arch rings sit within three centimetres of the collision wall.
    fn arch(&mut self, x: f32, z: f32) {
        let spring = 2.05;
        let inside = 1.50;
        let outside = 1.85;
        self.panel(
            Shutter,
            z + 0.013,
            1.,
            [x - inside, x + inside],
            [0.10, spring],
            Vec2::new(1.25, 1.),
        );
        for side in [-1., 1.] {
            let a = x + side * inside;
            let b = x + side * outside;
            for course in 0..5 {
                let lo = 0.10 + course as f32 * (spring - 0.10) / 5.;
                self.panel(
                    PaleStone,
                    z + 0.021,
                    1.,
                    [a.min(b), a.max(b)],
                    [lo, lo + (spring - 0.10) / 5. - 0.015],
                    Vec2::new(0.2, 0.13),
                );
            }
        }
        let center = Vec3::new(x, spring, z + 0.013);
        for i in 0..16 {
            let a = PI * i as f32 / 16.;
            let b = PI * (i + 1) as f32 / 16.;
            let point = |t: f32, r: f32, depth: f32| {
                Vec3::new(x + r * t.cos(), spring + r * t.sin(), z + depth)
            };
            let shutter = [
                center,
                point(a, inside, 0.013),
                point(b, inside, 0.013),
                center,
            ];
            let uv = shutter.map(|p| {
                Vec2::new(
                    (p.x - x + inside) / (2. * inside) * 1.25,
                    -(p.y - spring) / (spring - 0.1),
                )
            });
            self.batches[Shutter as usize].quad(shutter, uv, [1.; 4]);
            let r = if i == 7 || i == 8 {
                outside + 0.07
            } else {
                outside
            };
            let stones = [
                point(a + 0.007, inside, 0.022),
                point(a + 0.007, r, 0.022),
                point(b - 0.007, r, 0.022),
                point(b - 0.007, inside, 0.022),
            ];
            let uv = stones.map(|p| Vec2::new(p.x * 0.43, -p.y * 0.43));
            self.batches[PaleStone as usize].quad(stones, uv, [0.90 + 0.025 * (i % 4) as f32; 4]);
        }
        self.panel(
            Soot,
            z + 0.023,
            1.,
            [x - 0.28, x + 0.28],
            [0.92, 1.01],
            Vec2::ONE,
        );
    }
    fn pitched_roof(&mut self, x0: f32, x1: f32, y: f32, ridge: f32, z0: f32, z1: f32) {
        let m = (x0 + x1) * 0.5;
        let a = Vec3::new(x0, y, z0);
        let b = Vec3::new(m, ridge, z0);
        let c = Vec3::new(x1, y, z0);
        let dz = Vec3::Z * (z1 - z0);
        self.face(Brick, [a, b, c, a], [0.86; 4]);
        self.face(Brick, [a + dz, c + dz, b + dz, a + dz], [0.85; 4]);
        self.face(Soot, [a, a + dz, b + dz, b], [0.89; 4]);
        self.face(Soot, [b, b + dz, c + dz, c], [0.98; 4]);
    }
    fn workshop(&mut self, x: [f32; 2], depth: [f32; 2], height: f32, ridge: f32, facade: Surface) {
        let [x0, x1] = x;
        let [z0, z1] = depth;
        let facing = if z1 < 0. { 1. } else { -1. };
        let z = if facing > 0. { z1 } else { z0 };
        self.block(facade, PaleStone, [x0, 0., z0], [x1, height, z1]);
        self.pitched_roof(x0 - 0.23, x1 + 0.23, height, ridge, z0 - 0.19, z1 + 0.19);
        self.block(
            Soot,
            Iron,
            [x0 - 0.2, height - 0.15, z0 - 0.2],
            [x1 + 0.2, height + 0.01, z1 + 0.2],
        );
        let bays = ((x1 - x0) / 3.3).round().max(1.) as usize;
        let bay = (x1 - x0) / bays as f32;
        for i in 0..bays {
            let center = x0 + (i as f32 + 0.5) * bay;
            self.lintel_window(center, z, facing, height - 1.65, bay - 0.65, 1.29);
        }
    }
    fn round_section(&mut self, surface: Surface, center: Vec2, lower: [f32; 2], upper: [f32; 2]) {
        const SIDES: usize = 18;
        for i in 0..SIDES {
            let a = TAU * i as f32 / SIDES as f32;
            let b = TAU * (i + 1) as f32 / SIDES as f32;
            let p = |t: f32, yr: [f32; 2]| {
                Vec3::new(
                    center.x + t.cos() * yr[1],
                    yr[0],
                    center.y + t.sin() * yr[1],
                )
            };
            let pts = [p(b, lower), p(a, lower), p(a, upper), p(b, upper)];
            let u = (i as f32) % 4.;
            let h = (upper[0] - lower[0]) / 2.;
            self.batches[surface as usize].quad(
                pts,
                [
                    Vec2::new(u + 0.5, h),
                    Vec2::new(u, h),
                    Vec2::new(u, 0.),
                    Vec2::new(u + 0.5, 0.),
                ],
                [0.92, 0.92, 1., 1.],
            );
        }
    }
    fn tower(&mut self, x: f32, z: f32) {
        let center = Vec2::new(x, z);
        let footprint = [(-1.28, -1.28), (-1.28, 1.28), (1.28, 1.28), (1.28, -1.28)];
        for (i, (dx, dz)) in footprint.iter().copied().enumerate() {
            let (ex, ez) = footprint[(i + 1) % 4];
            self.beam(
                Soot,
                Vec3::new(x + dx * 1.34, 4.6, z + dz * 1.34),
                Vec3::new(x + dx, 9.55, z + dz),
                0.085,
            );
            self.beam(
                Iron,
                Vec3::new(x + dx * 1.15, 6.7, z + dz * 1.15),
                Vec3::new(x + ex, 9.4, z + ez),
                0.046,
            );
            self.beam(
                Iron,
                Vec3::new(x + ex * 1.15, 6.7, z + ez * 1.15),
                Vec3::new(x + dx, 9.4, z + dz),
                0.046,
            );
        }
        self.round_section(Soot, center, [9.03, 0.62], [9.57, 1.80]);
        self.round_section(Shutter, center, [9.55, 1.80], [11.66, 1.80]);
        for y in [9.57, 10.58, 11.61] {
            self.round_section(Iron, center, [y, 1.84], [y + 0.075, 1.84]);
        }
        self.round_section(Soot, center, [11.68, 1.91], [12.26, 0.06]);
        self.round_section(Iron, center, [12.24, 0.085], [12.50, 0.06]);
    }
    fn floor_label(&mut self, s: Surface, x: [f32; 2], z: [f32; 2], repeat: Vec2) {
        let p = corners(Vec3::new(x[0], 0.008, z[0]), Vec3::new(x[1], 0.008, z[1]));
        self.rectangle(s, p, repeat);
    }
    fn aim(&mut self) {
        // The north depot is an old limestone goods shed. The opposite end abuts a brick
        // generator shop. Both use the original wall footprint and have closed doors.
        for (z, facing) in [(-13.495, 1.), (13.495, -1.)] {
            self.panel(
                Slate,
                z,
                facing,
                [-14.48, 14.48],
                [0.0, 0.56],
                Vec2::new(7.3, 0.25),
            );
        }
        for x in [-4.35, 4.35] {
            self.arch(x, -13.5);
        }
        for x in [-10.65, 0., 10.65] {
            self.lintel_window(x, -13.5, 1., 3.52, 3.0, 1.20);
        }
        for x in [-4.4, 4.4] {
            self.panel(
                Soot,
                13.491,
                -1.,
                [x - 1.93, x + 1.93],
                [0.08, 3.5],
                Vec2::ONE,
            );
            self.panel(
                Shutter,
                13.482,
                -1.,
                [x - 1.79, x + 1.79],
                [0.10, 3.32],
                Vec2::new(1.45, 1.48),
            );
            self.panel(
                Iron,
                13.478,
                -1.,
                [x - 0.036, x + 0.036],
                [0.1, 3.32],
                Vec2::new(0.10, 1.6),
            );
            self.lintel_window(x, 13.5, -1., 3.68, 3.58, 0.98);
        }
        for x in [-10.6, 0., 10.6] {
            self.lintel_window(x, 13.5, -1., 3.66, 3.55, 1.0);
        }
        for sign in [-1.0_f32, 1.] {
            let x = sign * 14.5;
            for z in [-5., 5.] {
                let inward = -sign;
                let x = x + 0.012 * inward;
                let dz = 1.85 * inward;
                let pts = [
                    Vec3::new(x, 2.83, z + dz),
                    Vec3::new(x, 2.83, z - dz),
                    Vec3::new(x, 4.45, z - dz),
                    Vec3::new(x, 4.45, z + dz),
                ];
                self.rectangle(Window, pts, Vec2::new(1.5, 1.));
            }
            self.block(
                Slate,
                PaleStone,
                [x.min(sign * 15.02) - 0.015, 4.96, -14.05],
                [x.max(sign * 15.02) + 0.015, 5.12, 14.05],
            );
            self.block(
                PaleStone,
                PaleStone,
                [x.min(sign * 15.08) - 0.02, 5.22, -14.08],
                [x.max(sign * 15.08) + 0.02, 5.42, 14.08],
            );
            let z = sign * 13.5;
            self.block(
                Slate,
                PaleStone,
                [-14.52, 4.96, z.min(sign * 14.01) - 0.015],
                [14.52, 5.12, z.max(sign * 14.01) + 0.015],
            );
            self.block(
                PaleStone,
                PaleStone,
                [-14.60, 5.22, z.min(sign * 14.08) - 0.02],
                [14.60, 5.42, z.max(sign * 14.08) + 0.02],
            );
            for z in [-9.5, 0., 9.5] {
                self.block(
                    PaleStone,
                    PaleStone,
                    [x.min(x - sign * 0.14), 4.72, z - 0.40],
                    [x.max(x - sign * 0.14), 4.96, z + 0.40],
                );
            }
            let drain = sign * 12.65;
            self.floor_label(
                Drain,
                [drain - 0.12, drain + 0.12],
                [-12.8, 12.8],
                Vec2::new(8., 0.50),
            );
            for interval in [-9., -6., -3., 0., 3., 6., 9.] {
                let end = sign * 12.32;
                self.floor_label(
                    Hazard,
                    [end.min(end - sign * 0.55), end.max(end - sign * 0.55)],
                    [interval - 0.047, interval + 0.047],
                    Vec2::new(0.08, 0.55),
                );
            }
        }
        self.workshop([-8.7, 4.5], [-26., -17.6], 7.1, 8.7, Brick);
        self.workshop([4.7, 17.3], [-21.0, -15.9], 6.45, 8.05, Stone);
        self.tower(-10.0, -20.5);
        self.workshop([-17.7, -5.0], [16.2, 25.8], 7.45, 9.23, Brick);
        self.workshop([-4.7, 9.4], [15.6, 24.1], 7.95, 9.75, Brick);
        self.workshop([9.65, 19.6], [17.7, 25.9], 6.38, 7.88, Stone);
        for (x, z, h, r) in [(4.0, 23.8, 13.1, 0.61), (7., 25., 11.65, 0.49)] {
            let center = Vec2::new(x, z);
            self.round_section(Brick, center, [6., r * 1.34], [h, r]);
            self.round_section(Soot, center, [h - 0.27, r * 1.12], [h + 0.06, r * 1.12]);
        }
    }
    fn sky(&mut self) {
        const SLICES: usize = 40;
        const RINGS: usize = 12;
        let sphere = |theta: f32, phi: f32| {
            Vec3::new(theta.sin() * phi.cos(), phi.sin(), theta.cos() * phi.cos()) * 198.
        };
        for i in 0..SLICES {
            for j in 0..RINGS {
                let u = i as f32 / SLICES as f32;
                let v = j as f32 / RINGS as f32;
                let u2 = (i + 1) as f32 / SLICES as f32;
                let v2 = (j + 1) as f32 / RINGS as f32;
                let polar = |v: f32| -0.55 + v * (FRAC_PI_2 + 0.55);
                let st = |u: f32, v: f32| {
                    Vec2::new(
                        u * 2.,
                        (1. - (polar(v) / FRAC_PI_2).max(0.)).clamp(0.001, 0.997),
                    )
                };
                let p = [
                    sphere(u2 * TAU, polar(v)),
                    sphere(u * TAU, polar(v)),
                    sphere(u * TAU, polar(v2)),
                    sphere(u2 * TAU, polar(v2)),
                ];
                self.batches[Sky as usize].quad(
                    p,
                    [st(u2, v), st(u, v), st(u, v2), st(u2, v2)],
                    [1.; 4],
                );
            }
        }
    }
}

pub(crate) fn sync_map(
    mut commands: Commands,
    session: Res<Session>,
    materials: Res<EnvironmentMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    old: Query<Entity, With<WorldGeometry>>,
    mut generation: Local<u64>,
) {
    if *generation == session.map_revision {
        return;
    }
    for entity in &old {
        commands.entity(entity).despawn();
    }
    let mut yard = Yard::default();
    yard.collision_mesh(&session.map);
    if session.map.name == "aim" {
        yard.aim();
    }
    if matches!(
        session.map.name.as_str(),
        "aim" | "drill" | "relay" | "surf"
    ) {
        yard.sky();
    }
    for (i, part) in yard.batches.into_iter().enumerate() {
        if !part.indices.is_empty() {
            commands.spawn((
                Mesh3d(meshes.add(part.mesh())),
                MeshMaterial3d(materials.0[i].clone()),
                WorldGeometry,
            ));
        }
    }
    *generation = session.map_revision;
}
