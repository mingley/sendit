//! A compact, high-contrast match HUD. Diagnostic details and the control sheet are opt-in.
pub mod scoreboard;
use crate::{game::Session, options::Options};
use bevy::{prelude::*, window::PrimaryWindow};
use sendit_sim::{
    rules::{can_reach_objective, RoundResult, KIT_COST},
    BombView, Mode, Phase, TICK_RATE,
};
use std::time::{Duration, Instant};

fn ink() -> Color {
    Color::srgba(0.039, 0.066, 0.071, 0.92)
}
fn ash() -> Color {
    Color::srgb_u8(189, 205, 202)
}
fn amber() -> Color {
    Color::srgb_u8(255, 188, 93)
}

#[derive(Component, Clone, Copy)]
pub enum Field {
    Map,
    Link,
    Round,
    Health,
    Ammo,
    Rifle,
    Action,
    Status,
    Hints,
    Help,
    Stats,
    Cash,
    Armor,
}
#[derive(Component)]
pub struct Reticle;
#[derive(Component)]
pub struct Impact;

fn text(label: &str, size: f32, color: Color) -> impl Bundle {
    (
        Text::new(label),
        TextFont {
            font_size: size,
            ..default()
        },
        TextColor(color),
    )
}

fn card() -> Node {
    Node {
        padding: UiRect::axes(Val::Px(13.), Val::Px(7.)),
        align_items: AlignItems::Center,
        column_gap: Val::Px(12.),
        ..default()
    }
}

pub fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(22.),
                left: Val::Px(24.),
                border: UiRect::left(Val::Px(3.)),
                ..card()
            },
            BackgroundColor(ink()),
            BorderColor(amber()),
        ))
        .with_children(|row| {
            row.spawn(text("SENDIT", 20., Color::WHITE));
            row.spawn((text("", 14., ash()), Field::Map));
        });
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(24.),
                top: Val::Px(22.),
                ..card()
            },
            BackgroundColor(ink()),
        ))
        .with_children(|row| {
            row.spawn((text("", 14., ash()), Field::Link));
        });
    commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            top: Val::Px(22.),
            width: Val::Percent(100.),
            justify_content: JustifyContent::Center,
            ..default()
        })
        .with_children(|row| {
            row.spawn((
                text("", 18., Color::WHITE),
                Field::Round,
                BackgroundColor(ink()),
            ));
        });

    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(24.),
                bottom: Val::Px(24.),
                ..card()
            },
            BackgroundColor(ink()),
        ))
        .with_children(|row| {
            row.spawn(text("HP", 15., ash()));
            row.spawn((text("-", 33., Color::WHITE), Field::Health));
            row.spawn((text("", 14., ash()), Field::Armor));
            row.spawn((text("", 15., amber()), Field::Cash));
        });
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(24.),
                bottom: Val::Px(24.),
                ..card()
            },
            BackgroundColor(ink()),
        ))
        .with_children(|row| {
            row.spawn((text("RIFLE", 14., ash()), Field::Rifle));
            row.spawn((text("-", 33., Color::WHITE), Field::Ammo));
        });

    // Reticle center is empty and the halo stays legible on walls and uniforms.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.),
                height: Val::Percent(100.),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            Reticle,
        ))
        .with_children(|center| {
            center
                .spawn(Node {
                    width: Val::Px(19.),
                    height: Val::Px(19.),
                    ..default()
                })
                .with_children(|cross| {
                    for (x, y, w, h) in [
                        (8., 0., 3., 6.),
                        (8., 13., 3., 6.),
                        (0., 8., 6., 3.),
                        (13., 8., 6., 3.),
                    ] {
                        cross.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Px(x),
                                top: Val::Px(y),
                                width: Val::Px(w),
                                height: Val::Px(h),
                                border: UiRect::all(Val::Px(1.)),
                                ..default()
                            },
                            BackgroundColor(Color::srgb_u8(245, 255, 232)),
                            BorderColor(Color::srgb_u8(35, 49, 46)),
                        ));
                    }
                });
        });
    for (top, kind, size, color) in [
        (57., Field::Action, 16., amber()),
        (63., Field::Status, 20., Color::WHITE),
        (91., Field::Hints, 14., ash()),
    ] {
        commands
            .spawn(Node {
                position_type: PositionType::Absolute,
                top: Val::Percent(top),
                width: Val::Percent(100.),
                justify_content: JustifyContent::Center,
                ..default()
            })
            .with_children(|row| {
                row.spawn((text("", size, color), kind, BackgroundColor(ink())));
            });
    }
    commands.spawn((
        text("", 15., Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(24.),
            top: Val::Px(76.),
            ..default()
        },
        Field::Stats,
        BackgroundColor(ink()),
    ));
    commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.),
            top: Val::Percent(19.),
            justify_content: JustifyContent::Center,
            ..default()
        })
        .with_children(|row| {
            row.spawn((
                text("", 17., Color::WHITE),
                Field::Help,
                BackgroundColor(ink()),
            ));
        });
    scoreboard::spawn(&mut commands);
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::from_length(0.075))),
        MeshMaterial3d(mats.add(StandardMaterial {
            base_color: amber(),
            unlit: true,
            ..default()
        })),
        Transform::default(),
        Visibility::Hidden,
        Impact,
    ));
}

pub fn update(
    s: Res<Session>,
    opts: Res<Options>,
    mut fields: Query<(&Field, &mut Text)>,
    mut impact: Query<(&mut Transform, &mut Visibility), With<Impact>>,
    mut reticle: Query<&mut Visibility, (With<Reticle>, Without<Impact>)>,
    mut last: Local<Option<Instant>>,
) {
    let me = s.me();
    if let Ok(mut v) = reticle.get_single_mut() {
        v.set_if_neq(if me.is_some_and(|p| p.hp > 0) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
    if let Ok((mut transform, mut visibility)) = impact.get_single_mut() {
        let shot = if s.show_debug {
            me.and_then(|p| p.shot).or(s.local_shot)
        } else {
            s.local_shot
        };
        if let Some(shot) = shot.filter(|_| {
            s.show_debug
                || s.local_shot_time
                    .is_some_and(|t| t.elapsed().as_millis() < 150)
        }) {
            let distance = s.map.ray_distance(shot.origin, shot.direction, 120.);
            if distance < 120. {
                transform.translation = shot.origin + shot.direction * (distance - 0.04).max(0.);
                *visibility = Visibility::Inherited;
            } else {
                *visibility = Visibility::Hidden;
            }
        } else {
            visibility.set_if_neq(Visibility::Hidden);
        }
    }
    if last.is_some_and(|i| i.elapsed() < Duration::from_millis(60)) {
        return;
    }
    *last = Some(Instant::now());
    let mode = match s.mode {
        Mode::Practice => "AIM PRACTICE",
        Mode::Competitive => "COMPETITIVE",
        Mode::Deathmatch => "DEATHMATCH",
        Mode::Surf => "SURF",
    };
    for (field, mut label) in &mut fields {
        let value = match field {
            Field::Map => {
                if s.mode == Mode::Competitive {
                    let side = me.map_or("COMPETITIVE", |p| {
                        if p.team == 0 {
                            "T SIDE"
                        } else {
                            "CT SIDE"
                        }
                    });
                    format!("{}  /  {side}", s.map.name.to_uppercase())
                } else {
                    format!("{}  /  {mode}", s.map.name.to_uppercase())
                }
            }
            Field::Armor => me
                .filter(|p| p.armor > 0)
                .map_or_else(String::new, |p| format!("{} ARMOR", p.armor)),
            Field::Link => format!(
                "{} PLAYERS   |   {} ms",
                s.players.len(),
                s.rtt_ms.map_or_else(|| "-".into(), |v| format!("{v:.0}"))
            ),
            Field::Health => me.map_or_else(|| "-".into(), |p| p.hp.to_string()),
            Field::Cash => {
                if s.mode == Mode::Competitive {
                    me.map_or_else(String::new, |p| format!("${}", p.money))
                } else {
                    String::new()
                }
            }
            Field::Ammo => {
                if me.is_some_and(|p| p.armed && p.hp > 0) {
                    if s.mode == Mode::Competitive {
                        format!("{:02}/{:02}", s.rifle.ammo, s.rifle.magazine)
                    } else {
                        format!("{:02}", s.rifle.ammo)
                    }
                } else {
                    "-".into()
                }
            }
            Field::Rifle => if s.rifle.reload_left > 0 {
                "RELOADING"
            } else if me.is_some_and(|p| !p.armed) {
                "UNARMED"
            } else {
                "RIFLE"
            }
            .into(),
            Field::Round => {
                if s.mode == Mode::Competitive {
                    s.round.as_ref().map_or_else(String::new, |r| {
                        let phase = match r.phase {
                            Phase::Warmup => "WARMUP",
                            Phase::Buy => "BUY",
                            Phase::Live => "LIVE",
                            Phase::Planted => "PLANTED",
                            Phase::Ended => "ROUND OVER",
                        };
                        let secs = r.remaining.div_ceil(TICK_RATE);
                        format!(
                            "   T  {}     {:02}:{:02}     {}  CT   |   {phase}   ",
                            r.score[0],
                            secs / 60,
                            secs % 60,
                            r.score[1]
                        )
                    })
                } else {
                    String::new()
                }
            }
            Field::Action => {
                if s.mode == Mode::Competitive && s.round.is_some_and(|r| r.phase == Phase::Ended) {
                    ended(&s)
                } else if me.is_some_and(|p| p.hp == 0) {
                    if let Some(who) = s.spectate {
                        format!("   Spectating {}   |   Tap Tab to switch   ", s.name(who))
                    } else {
                        "   Eliminated   ".into()
                    }
                } else if s.hit_at.is_some_and(|i| i.elapsed().as_secs_f32() < 0.8) {
                    "   Elimination   ".into()
                } else if s
                    .damage_at
                    .is_some_and(|i| i.elapsed().as_secs_f32() < 0.45)
                {
                    "   Hit   ".into()
                } else if s.mode == Mode::Competitive {
                    objective(&s)
                } else {
                    String::new()
                }
            }
            Field::Status => {
                if s.scoreboard {
                    String::new()
                } else if !s.status.is_empty() {
                    format!("   {}   ", s.status)
                } else if !s.captured
                    && s.replay.is_none()
                    && (opts.benchmark.is_none() || opts.live_input)
                {
                    "   Click to capture mouse   ".into()
                } else {
                    String::new()
                }
            }
            Field::Hints => {
                if s.show_help || s.scoreboard {
                    String::new()
                } else if matches!(s.mode, Mode::Practice | Mode::Surf) {
                    "   F1  Controls     F3  Telemetry     F5  Reset   ".into()
                } else {
                    "   Tab  Scoreboard     F1  Controls   ".into()
                }
            }
            Field::Stats => {
                if s.show_debug && !s.scoreboard {
                    telemetry(&s)
                } else {
                    String::new()
                }
            }
            Field::Help => {
                if s.show_help && !s.scoreboard {
                    "   CONTROLS\n\n   WASD     Move             Mouse     Aim\n   Space     Jump             LMB         Fire\n   R             Reload          Esc           Release mouse\n   E             Plant / defuse (hold) or recover\n   G             Drop bomb    B              Buy rifle\n   Tab         Hold for scores, tap to cycle spectator\n   F3           Shot telemetry\n   F5           Reset practice or surf\n\n   F1           Close   ".into()
                } else {
                    String::new()
                }
            }
        };
        if **label != value {
            **label = value;
        }
    }
}

pub fn scale(windows: Query<&Window, With<PrimaryWindow>>, mut ui: ResMut<UiScale>) {
    if let Ok(window) = windows.get_single() {
        let scale = (window.width() / 1060.)
            .min(window.height() / 610.)
            .clamp(0.53, 1.0);
        if ui.0 != scale {
            ui.0 = scale;
        }
    }
}

fn ended(s: &Session) -> String {
    let Some(r) = s.round else {
        return String::new();
    };
    let Some(winner) = r.winner else {
        return "   Round complete   ".into();
    };
    let team = if winner == 0 {
        "Terrorists"
    } else {
        "Counter-Terrorists"
    };
    let reason = match r.result {
        Some(RoundResult::Defused) => "bomb defused",
        Some(RoundResult::Detonated) => "bomb detonated",
        Some(RoundResult::Elimination) => "elimination",
        Some(RoundResult::Time) => "time expired",
        None => "round complete",
    };
    let event = if r.match_over { "match" } else { "round" };
    format!("   {team} win the {event}   /   {reason}   ")
}

fn objective(s: &Session) -> String {
    let (Some(r), Some(p)) = (s.round, s.me()) else {
        return String::new();
    };
    if r.phase == Phase::Warmup {
        return "   Warmup   /   Waiting for both teams   ".into();
    }
    if r.phase == Phase::Buy {
        if p.rifle.magazine == sendit_sim::rifle::MAGAZINE
            && p.armor == sendit_sim::Loadout::Armored.armor()
        {
            return "   Rifle kit equipped   ".into();
        }
        if p.money < KIT_COST {
            return format!(
                "   Rifle kit  ${KIT_COST}   /   Need ${}   ",
                KIT_COST - p.money
            );
        }
        return format!("   B   Buy rifle kit + 50 armor   ${KIT_COST}   ");
    }
    if r.action_by == Some(p.id) && r.progress > 0. {
        let verb = if r.phase == Phase::Planted {
            "Defusing"
        } else {
            "Planting"
        };
        return format!(
            "   {verb}    {:3.0}%   |   Hold E   ",
            (r.progress * 100.).clamp(0., 100.)
        );
    }
    let prompt = match r.bomb {
        BombView::Carried(id) if id == p.id => {
            if let Some(site) = s
                .map
                .sites
                .iter()
                .find(|site| site.pos.distance(p.body.pos) <= site.radius)
            {
                format!("Site {}   |   Hold E while still to plant", site.name)
            } else {
                "Carrying bomb   |   G  Drop".into()
            }
        }
        BombView::Carried(id) if p.team == 0 => format!("Bomb carrier   {}", s.name(id)),
        BombView::Dropped(pos) if p.team == 0 && can_reach_objective(&s.map, &p.body, pos) => {
            "E   Recover bomb".into()
        }
        BombView::Dropped(_) if p.team == 0 => "Bomb dropped".into(),
        BombView::Planted(idx, pos) => {
            let site = s
                .map
                .sites
                .get(idx as usize)
                .map_or("?", |s| s.name.as_str());
            if p.team == 1 && can_reach_objective(&s.map, &p.body, pos) {
                format!("Site {site}   |   Hold E while still to defuse")
            } else {
                format!("Bomb planted   /   Site {site}")
            }
        }
        _ => return String::new(),
    };
    format!("   {prompt}   ")
}

fn telemetry(s: &Session) -> String {
    let v = s.body.vel;
    let ready = s.rifle.accurate(&s.body, &s.tuning);
    let mut stats = format!(
        "   MOVEMENT\n   Velocity     {:+.2}  {:+.2}  {:+.2} m/s\n   Speed         {:.2} m/s\n   First shot    {}   ",
        v.x,
        v.y,
        v.z,
        s.body.speed(),
        if ready { "settled" } else { "recovering" }
    );
    if let Some((p, shot)) = s.me().and_then(|p| p.shot.map(|shot| (p, shot))) {
        use std::fmt::Write;
        let d = shot.direction;
        let _ = write!(
            stats,
            "\n\n   SERVER SHOT   #{}\n   Direction     {:+.4}  {:+.4}  {:+.4}\n   Velocity       {:.2} m/s\n   Recoil          {:.3} deg\n   Spread        {:.3} deg\n   Rewind       {} ticks   ",
            p.shot_tick,
            d.x,
            d.y,
            d.z,
            shot.velocity.length(),
            shot.recoil.to_degrees(),
            shot.spread.to_degrees(),
            p.rewind_ticks
        );
    }
    stats
}
