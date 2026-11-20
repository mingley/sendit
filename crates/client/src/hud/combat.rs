//! Screen-space sights and combat feedback. Damage directions use the source
//! of the confirmed hit. The thinner amber signal comes from audible gunfire.
use crate::{game::Session, menu::MenuState};
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    ui::FocusPolicy,
};
use sendit_sim::Mode;
use std::time::Instant;

#[derive(Component, Clone, Copy)]
pub enum Layer {
    Sights,
    Scope,
}

#[derive(Component, Clone, Copy)]
pub enum Mark {
    Aim(Vec2),
    Confirm(Vec2),
    Damage(f32),
    Gunfire(f32),
    ScopeDot,
}

const MASK_INK: [u8; 4] = [8, 9, 6, 249];

fn scope_mask() -> Image {
    const SIDE: u32 = 256;
    let mut data = Vec::with_capacity((SIDE * SIDE * 4) as usize);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let xy = (Vec2::new(x as f32, y as f32) + Vec2::splat(0.5)) / (SIDE as f32 * 0.5)
                - Vec2::ONE;
            let radius = xy.length();
            let edge = ((radius - 0.925) / 0.028).clamp(0., 1.);
            let alpha = (f32::from(MASK_INK[3]) * edge * edge) as u8;
            data.extend_from_slice(&[MASK_INK[0], MASK_INK[1], MASK_INK[2], alpha]);
        }
    }
    Image::new(
        Extent3d {
            width: SIDE,
            height: SIDE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}

fn scope(commands: &mut Commands, images: &mut Assets<Image>) {
    let [r, g, b, a] = MASK_INK;
    let shade = BackgroundColor(Color::srgba_u8(r, g, b, a));
    let fill = Node {
        flex_grow: 1.,
        ..default()
    };
    commands
        .spawn((
            Layer::Scope,
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                width: Val::Percent(100.),
                height: Val::Percent(100.),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Stretch,
                ..default()
            },
            GlobalZIndex(-1),
            FocusPolicy::Pass,
        ))
        .with_children(|screen| {
            screen.spawn((fill.clone(), shade, FocusPolicy::Pass));
            screen
                .spawn(Node {
                    width: Val::Percent(100.),
                    height: Val::VMin(88.),
                    flex_shrink: 0.,
                    ..default()
                })
                .with_children(|middle| {
                    middle.spawn((fill.clone(), shade, FocusPolicy::Pass));
                    middle
                        .spawn((
                            Node {
                                width: Val::VMin(88.),
                                height: Val::Percent(100.),
                                flex_shrink: 0.,
                                ..default()
                            },
                            ImageNode::new(images.add(scope_mask())),
                            FocusPolicy::Pass,
                        ))
                        .with_children(|lens| {
                            for (x, y, width, height) in [
                                (2., 50., Val::Percent(96.), Val::Px(1.4)),
                                (50., 2., Val::Px(1.4), Val::Percent(96.)),
                            ] {
                                lens.spawn((
                                    Node {
                                        position_type: PositionType::Absolute,
                                        left: Val::Percent(x),
                                        top: Val::Percent(y),
                                        width,
                                        height,
                                        ..default()
                                    },
                                    BackgroundColor(Color::srgba_u8(26, 27, 21, 224)),
                                    FocusPolicy::Pass,
                                ));
                            }
                            for offset in [-12., -6., 6., 12.] {
                                for (x, y) in [(50. + offset, 50.), (50., 50. + offset)] {
                                    lens.spawn((
                                        Node {
                                            position_type: PositionType::Absolute,
                                            left: Val::Percent(x),
                                            top: Val::Percent(y),
                                            width: Val::Px(3.),
                                            height: Val::Px(3.),
                                            margin: UiRect::axes(Val::Px(-0.8), Val::Px(-0.8)),
                                            ..default()
                                        },
                                        BorderRadius::MAX,
                                        BackgroundColor(Color::srgb_u8(32, 32, 24)),
                                        FocusPolicy::Pass,
                                    ));
                                }
                            }
                        });
                    middle.spawn((fill.clone(), shade, FocusPolicy::Pass));
                });
            screen.spawn((fill, shade, FocusPolicy::Pass));
        });
}

pub fn spawn(commands: &mut Commands, images: &mut Assets<Image>) {
    scope(commands, images);
    commands
        .spawn((
            Layer::Sights,
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                width: Val::Percent(100.),
                height: Val::Percent(100.),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            GlobalZIndex(3),
            FocusPolicy::Pass,
        ))
        .with_children(|screen| {
            screen
                .spawn((
                    Node {
                        width: Val::Px(0.),
                        height: Val::Px(0.),
                        ..default()
                    },
                    FocusPolicy::Pass,
                ))
                .with_children(|center| {
                    let marks = [Vec2::NEG_Y, Vec2::X, Vec2::Y, Vec2::NEG_X]
                        .map(Mark::Aim)
                        .into_iter()
                        .chain(
                            [
                                Vec2::new(-1., -1.),
                                Vec2::new(1., -1.),
                                Vec2::new(-1., 1.),
                                Vec2::ONE,
                            ]
                            .map(Mark::Confirm),
                        )
                        .chain([
                            Mark::Damage(-1.),
                            Mark::Damage(1.),
                            Mark::Gunfire(-1.),
                            Mark::Gunfire(1.),
                            Mark::ScopeDot,
                        ]);
                    for mark in marks {
                        center.spawn((
                            mark,
                            Node {
                                position_type: PositionType::Absolute,
                                border: UiRect::all(Val::Px(0.65)),
                                ..default()
                            },
                            BorderColor(Color::srgba_u8(35, 29, 18, 220)),
                            BackgroundColor(super::amber()),
                            Visibility::Hidden,
                            FocusPolicy::Pass,
                        ));
                    }
                });
        });
}

fn intensity(at: Option<Instant>, now: Instant, duration: f32) -> f32 {
    at.map_or(0., |t| {
        (1. - now.saturating_duration_since(t).as_secs_f32() / duration).clamp(0., 1.)
    })
}

/// Screen up is forward. Positive view yaw turns from world -Z toward +X.
fn bearing(position: Vec3, source: Vec3, yaw: f32) -> Option<Vec2> {
    let relative = source - position;
    let world = Vec2::new(relative.x, relative.z).try_normalize()?;
    let (s, c) = yaw.sin_cos();
    Some(Vec2::new(
        world.x * c + world.y * s,
        -world.x * s + world.y * c,
    ))
}

#[derive(Clone, Copy)]
struct Stroke {
    a: Vec2,
    b: Vec2,
    width: f32,
    color: Color,
}

impl Stroke {
    fn radial(dir: Vec2, start: f32, end: f32, width: f32, color: Color) -> Self {
        Self {
            a: dir * start,
            b: dir * end,
            width,
            color,
        }
    }

    fn place(self, node: &mut Mut<Node>, transform: &mut Mut<Transform>) {
        let axis = self.b - self.a;
        let size = Vec2::new(axis.length(), self.width);
        let top_left = (self.a + self.b - size) * 0.5;
        let (left, top, width, height) = (
            Val::Px(top_left.x),
            Val::Px(top_left.y),
            Val::Px(size.x),
            Val::Px(size.y),
        );
        if (node.left, node.top, node.width, node.height) != (left, top, width, height) {
            node.left = left;
            node.top = top;
            node.width = width;
            node.height = height;
        }
        let rotation = Quat::from_rotation_z(axis.y.atan2(axis.x));
        if transform.rotation != rotation {
            transform.rotation = rotation;
        }
    }
}

pub fn update(
    s: Res<Session>,
    menu: Res<MenuState>,
    mut layers: Query<(&Layer, &mut Node), Without<Mark>>,
    mut marks: Query<
        (
            &Mark,
            &mut Node,
            &mut Transform,
            &mut BackgroundColor,
            &mut Visibility,
        ),
        Without<Layer>,
    >,
) {
    let now = Instant::now();
    let me = s.me();
    let alive = me.is_some_and(|p| p.hp > 0);
    let damage = intensity(s.damage_at, now, 0.65);
    // The death camera holds the victim's final facing until spectating or respawn.
    let last_hit = me.filter(|p| p.hp == 0 && s.spectate.is_none() && damage > 0.);
    let show = !menu.open
        && !s.local_lobby
        && !s.scoreboard
        && !s.show_help
        && (alive || last_hit.is_some());
    let scoped = show && alive && s.scoped();
    for (layer, mut node) in &mut layers {
        let visible = match layer {
            Layer::Sights => show,
            Layer::Scope => scoped,
        };
        let display = if visible {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
    if !show {
        return;
    }

    let fired = intensity(s.local_shot_time, now, 0.21);
    let hit = intensity(s.confirmed_hit_at, now, 0.34);
    let recent_fire = s
        .nearby_fire
        .filter(|(t, _)| intensity(Some(*t), now, 0.36) > 0.);
    let damage_dir = s
        .damage_origin
        .and_then(|origin| bearing(s.body.pos, origin, last_hit.map_or(s.input.yaw, |p| p.yaw)));
    let nearby_dir = recent_fire.and_then(|(_, origin)| bearing(s.body.pos, origin, s.input.yaw));
    let motion = if s.mode == Mode::Surf {
        0.
    } else {
        (s.body.speed() / s.tuning.run_speed.max(0.1)).min(1.)
    };
    let gap = 4.3 + 3.0 * motion + 9.0 * fired;
    for (mark, mut node, mut transform, mut color, mut visibility) in &mut marks {
        let stroke = match *mark {
            Mark::Aim(dir) if alive && !scoped => {
                let color = if fired > 0.45 {
                    Color::srgb_u8(255, 245, 193)
                } else {
                    super::amber()
                };
                Some(Stroke::radial(dir, gap, gap + 8. + 4. * fired, 3.4, color))
            }
            Mark::Confirm(dir) if hit > 0. => {
                let dir = dir.normalize();
                let alpha = (hit * 3.).min(1.);
                Some(Stroke::radial(
                    dir,
                    7.,
                    17. + 3. * hit,
                    4.,
                    Color::srgba(1., 1., 0.92, alpha),
                ))
            }
            Mark::Damage(side) if damage > 0. => damage_dir.map(|dir| {
                let tangent = Vec2::new(-dir.y, dir.x);
                let tip = dir * 76.;
                Stroke {
                    a: tip,
                    b: tip - dir * 8. + tangent * side * 19.,
                    width: 7.,
                    color: Color::srgba(1., 0.27, 0.16, (damage * 2.5).min(1.)),
                }
            }),
            Mark::Gunfire(side) if alive && damage == 0. => nearby_dir.map(|dir| {
                let tangent = Vec2::new(-dir.y, dir.x);
                let center = dir * 85.;
                Stroke {
                    a: center + tangent * side * 4.,
                    b: center + tangent * side * 16.,
                    width: 3.3,
                    color: super::amber(),
                }
            }),
            Mark::ScopeDot if scoped => Some(Stroke::radial(
                Vec2::X,
                -1.8 - fired,
                1.8 + fired,
                3.6 + 2. * fired,
                super::amber(),
            )),
            _ => None,
        };
        if let Some(stroke) = stroke {
            stroke.place(&mut node, &mut transform);
            color.set_if_neq(BackgroundColor(stroke.color));
            visibility.set_if_neq(Visibility::Inherited);
        } else {
            visibility.set_if_neq(Visibility::Hidden);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::Options;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn a_missed_shot_audible_fire_and_a_confirmed_hit_show_different_marks() {
        let opts = Options {
            local_match: false,
            ..default()
        };
        let mut session = Session::open(&opts).unwrap();
        let mut world = sendit_sim::World::new(session.map.clone(), Mode::Practice, session.tuning);
        assert!(world.add(1));
        session.id = Some(1);
        session.players = world.views();
        session.body = session.players[0].body;
        let source = session.body.pos + Vec3::X;
        let mut app = App::new();
        app.insert_resource(session)
            .init_resource::<MenuState>()
            .add_systems(Update, update);
        let [aim, hit, damage, gunfire] = [
            Mark::Aim(Vec2::X),
            Mark::Confirm(Vec2::ONE),
            Mark::Damage(1.),
            Mark::Gunfire(1.),
        ]
        .map(|mark| {
            app.world_mut()
                .spawn((
                    mark,
                    Node::default(),
                    BackgroundColor::default(),
                    Visibility::Hidden,
                ))
                .id()
        });
        let visible =
            |app: &App, e: Entity| *app.world().get::<Visibility>(e).unwrap() != Visibility::Hidden;
        let gap = |app: &App| match app.world().get::<Node>(aim).unwrap().left {
            Val::Px(value) => value,
            _ => panic!("reticle gap should use logical pixels"),
        };
        app.update();
        let resting = gap(&app);
        app.world_mut().resource_mut::<Session>().local_shot_time = Some(Instant::now());
        app.update();
        assert!(gap(&app) > resting + 5. && !visible(&app, hit));

        app.world_mut().resource_mut::<Session>().nearby_fire = Some((Instant::now(), source));
        app.update();
        assert!(visible(&app, gunfire) && !visible(&app, damage) && !visible(&app, hit));

        let mut session = app.world_mut().resource_mut::<Session>();
        session.damage_at = Some(Instant::now());
        session.damage_origin = Some(source);
        session.confirmed_hit_at = Some(Instant::now());
        app.update();
        assert!(visible(&app, damage) && visible(&app, hit) && !visible(&app, gunfire));

        let mut session = app.world_mut().resource_mut::<Session>();
        session.players[0].hp = 0;
        session.players[0].yaw = FRAC_PI_2;
        app.update();
        assert!(visible(&app, damage) && !visible(&app, aim));
        assert!(matches!(app.world().get::<Node>(damage).unwrap().top, Val::Px(y) if y < -60.));
    }

    #[test]
    fn reported_damage_origin_stays_relative_to_the_current_view() {
        let me = Vec3::new(4., 0., 6.);
        let north = me + Vec3::NEG_Z * 8.;
        assert_eq!(bearing(me, north, 0.), Some(Vec2::NEG_Y));
        assert_eq!(bearing(me, me + Vec3::X, 0.), Some(Vec2::X));
        assert!(bearing(me, north, FRAC_PI_2).unwrap().distance(Vec2::NEG_X) < 0.001);
        assert!(
            bearing(me, north, std::f32::consts::PI)
                .unwrap()
                .distance(Vec2::Y)
                < 0.001
        );
        assert_eq!(bearing(me, me + Vec3::Y, 0.), None);
    }
}
