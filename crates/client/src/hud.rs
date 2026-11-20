//! A compact, high-contrast match HUD. Diagnostic details and the control sheet are opt-in.
pub mod arsenal;
pub mod combat;
mod course;
pub mod scoreboard;
use crate::{game::Session, options::Options};
use bevy::{prelude::*, window::PrimaryWindow};
use sendit_sim::{
    rules::{can_reach_objective, RoundResult, KIT_COST},
    BombView, Mode, Phase, TICK_RATE,
};
use std::time::{Duration, Instant};

pub(crate) fn ink() -> Color {
    Color::srgba(0.085, 0.079, 0.058, 0.92)
}
pub(crate) fn ash() -> Color {
    Color::srgb_u8(222, 210, 184)
}
pub(crate) fn amber() -> Color {
    Color::srgb_u8(255, 194, 95)
}

#[derive(Component, Clone, Copy)]
pub enum Field {
    Map,
    Link,
    Round,
    Health,
    VitalLabel,
    Ammo,
    Rifle,
    Action,
    Status,
    Hints,
    Help,
    Stats,
    Cash,
    Armor,
    LobbyBots,
    LobbySkill,
    LobbyControls,
    LobbyAction,
}

#[derive(Component, Clone, Copy)]
pub(crate) enum HudLayer {
    Play,
    Lobby,
}
#[derive(Component)]
pub struct Impact;

pub(crate) fn text(label: &str, size: f32, color: Color) -> impl Bundle {
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

fn spawn_local_lobby(commands: &mut Commands) {
    commands
        .spawn((
            HudLayer::Lobby,
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                width: Val::Percent(100.),
                height: Val::Percent(100.),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            GlobalZIndex(12),
            BackgroundColor(Color::srgba(0.025, 0.045, 0.05, 0.34)),
        ))
        .with_children(|screen| {
            screen
                .spawn((
                    Node {
                        width: Val::Px(452.),
                        max_width: Val::Percent(88.),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(12.),
                        border: UiRect::left(Val::Px(4.)),
                        padding: UiRect::all(Val::Px(24.)),
                        ..default()
                    },
                    BorderColor(amber()),
                    BackgroundColor(ink()),
                ))
                .with_children(|panel| {
                    panel.spawn(text("Play bot deathmatch", 29., Color::WHITE));
                    for (value, hint) in [
                        (Field::LobbyBots, "Left / Right arrows"),
                        (Field::LobbySkill, "Down / Up arrows"),
                    ] {
                        panel
                            .spawn(Node {
                                justify_content: JustifyContent::SpaceBetween,
                                align_items: AlignItems::Center,
                                column_gap: Val::Px(12.),
                                ..default()
                            })
                            .with_children(|row| {
                                row.spawn((text("", 18., Color::WHITE), value));
                                row.spawn(text(hint, 16., ash()));
                            });
                    }
                    panel.spawn((text("", 16., ash()), Field::LobbyControls));
                    panel
                        .spawn(Node {
                            justify_content: JustifyContent::SpaceBetween,
                            column_gap: Val::Px(12.),
                            margin: UiRect::top(Val::Px(5.)),
                            ..default()
                        })
                        .with_children(|actions| {
                            actions
                                .spawn((
                                    Node {
                                        align_items: AlignItems::Center,
                                        padding: UiRect::axes(Val::Px(16.), Val::Px(8.)),
                                        ..default()
                                    },
                                    BackgroundColor(amber()),
                                ))
                                .with_children(|button| {
                                    button.spawn((
                                        text("", 18., Color::srgb_u8(32, 30, 22)),
                                        Field::LobbyAction,
                                    ));
                                });
                            crate::menu::button(
                                actions,
                                crate::menu::Choice::LobbyQuit,
                                "Q   Quit",
                            );
                        });
                });
        });
}

pub fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    commands
        .spawn((
            HudLayer::Play,
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
            HudLayer::Play,
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
        .spawn((
            HudLayer::Play,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(22.),
                width: Val::Percent(100.),
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|row| {
            row.spawn((
                text("", 18., Color::WHITE),
                Field::Round,
                BackgroundColor(ink()),
            ));
        });

    commands
        .spawn((
            HudLayer::Play,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(24.),
                bottom: Val::Px(24.),
                ..card()
            },
            BackgroundColor(ink()),
        ))
        .with_children(|row| {
            row.spawn((text("HP", 15., ash()), Field::VitalLabel));
            row.spawn((text("-", 33., amber()), Field::Health));
            row.spawn((text("", 14., ash()), Field::Armor));
            row.spawn((text("", 15., amber()), Field::Cash));
        });
    commands
        .spawn((
            HudLayer::Play,
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(24.),
                bottom: Val::Px(24.),
                ..card()
            },
            BackgroundColor(ink()),
        ))
        .with_children(|row| {
            row.spawn((text("", 15., ash()), Field::Rifle));
            row.spawn((text("-", 32., amber()), Field::Ammo));
        });

    combat::spawn(&mut commands, &mut images);
    arsenal::spawn(&mut commands);
    for (top, kind, size, color) in [
        (57., Field::Action, 16., amber()),
        (63., Field::Status, 20., Color::WHITE),
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
        text("", 13., ash()),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(24.),
            top: Val::Px(66.),
            ..default()
        },
        Field::Hints,
        BackgroundColor(ink()),
    ));
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
    spawn_local_lobby(&mut commands);
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

#[derive(Default)]
pub(crate) struct ReturnNotice {
    dead: Option<(u16, u32)>,
    respawned: Option<Instant>,
}

impl ReturnNotice {
    fn observe(&mut self, player: Option<&sendit_sim::PlayerView>, now: Instant) -> bool {
        let Some(me) = player else {
            self.dead = None;
            self.respawned = None;
            return false;
        };
        if me.hp == 0 {
            self.dead = Some((me.id, me.life));
            self.respawned = None;
        } else if let Some((id, life)) = self.dead.take() {
            if id == me.id && life != me.life {
                self.respawned = Some(now);
            }
        }
        self.respawned
            .is_some_and(|t| now.saturating_duration_since(t) < Duration::from_millis(600))
    }
}

pub fn update(
    s: Res<Session>,
    opts: Res<Options>,
    mut fields: Query<(&Field, &mut Text, &mut TextFont)>,
    mut layers: Query<(&HudLayer, &mut Node)>,
    mut impact: Query<(&mut Transform, &mut Visibility), With<Impact>>,
    menu: Res<crate::menu::MenuState>,
    mut last: Local<Option<Instant>>,
    mut return_notice: Local<ReturnNotice>,
) {
    let me = s.me();
    let lobby = s.local_lobby;
    let back_in_play =
        return_notice.observe(me.filter(|_| s.mode == Mode::Deathmatch), Instant::now());
    for (layer, mut node) in &mut layers {
        let shown = !menu.open && matches!(layer, HudLayer::Lobby) == lobby;
        let display = if shown { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    if let Ok((mut transform, mut visibility)) = impact.get_single_mut() {
        let shot = me.and_then(|p| p.shot).or(s.local_shot);
        if let Some(shot) = shot.filter(|_| s.show_debug && !menu.open && !lobby) {
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
    if !menu.is_changed() && last.is_some_and(|i| i.elapsed() < Duration::from_millis(60)) {
        return;
    }
    *last = Some(Instant::now());
    let mode = match s.mode {
        Mode::Practice => "AIM PRACTICE",
        Mode::Competitive => "COMPETITIVE",
        Mode::Deathmatch => "FREE-FOR-ALL",
        Mode::Surf => "SURF",
    };
    let course = if s.mode == Mode::Surf {
        course::readout(&s.map.course, &s.body, s.input.yaw)
    } else {
        Default::default()
    };
    for (field, mut label, mut font) in &mut fields {
        let deathmatch = s.mode == Mode::Deathmatch;
        let value = match field {
            _ if menu.open => String::new(),
            Field::LobbyBots if lobby => format!(
                "{} {}",
                opts.bots,
                if opts.bots == 1 { "bot" } else { "bots" }
            ),
            Field::LobbySkill if lobby => format!("Skill {:.0}%", opts.bot_skill * 100.),
            Field::LobbyControls if lobby => {
                if s.status.is_empty() {
                    "WASD move    Mouse aim\nLeft click fire    Right click scope\n1-6 weapons    R reload".into()
                } else {
                    s.status.clone()
                }
            }
            Field::LobbyAction if lobby => if !s.scene_ready {
                "Loading arena..."
            } else if !s.status.is_empty() {
                "Click to retry"
            } else {
                "Click to play"
            }
            .into(),
            Field::LobbyBots | Field::LobbySkill | Field::LobbyControls | Field::LobbyAction => {
                String::new()
            }
            _ if lobby => String::new(),
            Field::Map => {
                if s.id.is_none() {
                    "CONNECTING".into()
                } else if s.map.name.eq_ignore_ascii_case(mode) {
                    mode.into()
                } else if s.mode == Mode::Competitive {
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
            Field::Link if s.mode == Mode::Deathmatch && me.is_some() => {
                let rivals = s.players.len().saturating_sub(1);
                format!(
                    "{rivals} {}   |   {} ms",
                    if rivals == 1 { "RIVAL" } else { "RIVALS" },
                    s.rtt_ms.map_or_else(|| "-".into(), |v| format!("{v:.0}"))
                )
            }
            Field::Link => format!(
                "{} {}   |   {} ms",
                s.players.len(),
                if s.players.len() == 1 {
                    "PLAYER"
                } else {
                    "PLAYERS"
                },
                s.rtt_ms.map_or_else(|| "-".into(), |v| format!("{v:.0}"))
            ),
            Field::VitalLabel => if s.mode == Mode::Surf {
                "CONTACT"
            } else {
                "HP"
            }
            .into(),
            Field::Health if s.mode == Mode::Surf => if s.body.surfing {
                "BANK"
            } else if s.body.grounded {
                "GROUND"
            } else {
                "AIR"
            }
            .into(),
            Field::Health => me.map_or_else(|| "-".into(), |p| p.hp.to_string()),
            Field::Cash => {
                if s.mode == Mode::Competitive {
                    me.map_or_else(String::new, |p| format!("${}", p.money))
                } else {
                    String::new()
                }
            }
            Field::Ammo if s.mode == Mode::Surf => format!("{:.1} m/s", s.body.vel.length()),
            Field::Ammo => {
                let viewed = s
                    .spectate
                    .and_then(|id| s.players.iter().find(|p| p.id == id))
                    .or(me);
                if viewed.is_some_and(|p| p.hp > 0) {
                    let rifle = s.view_rifle();
                    if rifle.weapon == sendit_sim::Weapon::Knife {
                        String::new()
                    } else {
                        format!("{:02}/{:02}", rifle.ammo, rifle.magazine)
                    }
                } else {
                    "-".into()
                }
            }
            Field::Rifle if s.mode == Mode::Surf => "SPEED".into(),
            Field::Rifle => {
                let rifle = s.view_rifle();
                let name = rifle.weapon.label().to_uppercase();
                if rifle.reload_left > 0 {
                    format!("{name}\nRELOADING")
                } else if rifle.switch_left > 0 {
                    format!("{name}\nDRAWING")
                } else if rifle.weapon != sendit_sim::Weapon::Knife {
                    format!("{name}\nMAGAZINE")
                } else {
                    format!("{name}\nMELEE")
                }
            }
            Field::Round if s.mode == Mode::Deathmatch => me.map_or_else(String::new, |p| {
                format!(
                    "   {} {}   ",
                    p.kills,
                    if p.kills == 1 { "KILL" } else { "KILLS" }
                )
            }),
            Field::Round if s.mode == Mode::Surf => course.0.clone(),
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
            Field::Action if s.scoreboard || s.show_help => String::new(),
            Field::Action if s.mode == Mode::Deathmatch => {
                let killed = s
                    .hit_at
                    .is_some_and(|t| t.elapsed() < Duration::from_millis(1050));
                let damaged = s
                    .damage_at
                    .is_some_and(|t| t.elapsed() < Duration::from_millis(650));
                let notice = me.map_or("", |p| {
                    deathmatch_notice(p.hp, p.kills, killed, damaged, back_in_play)
                });
                if notice.is_empty() {
                    nearby_fire_notice(&s).into()
                } else {
                    notice.into()
                }
            }
            Field::Action if s.mode == Mode::Surf => course.1.clone(),
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
                    .is_some_and(|i| i.elapsed() < Duration::from_millis(650))
                {
                    "   Hit   ".into()
                } else if s.mode == Mode::Competitive {
                    let prompt = objective(&s);
                    if prompt.is_empty() {
                        nearby_fire_notice(&s).into()
                    } else {
                        prompt
                    }
                } else {
                    nearby_fire_notice(&s).into()
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
                if s.show_help || s.show_debug || s.scoreboard {
                    String::new()
                } else if s.mode == Mode::Surf {
                    "  Esc Menu    F5 Restart run  ".into()
                } else if s.mode == Mode::Practice {
                    "  Esc Menu    F5 Reset  ".into()
                } else {
                    "  Esc Menu  ".into()
                }
            }
            Field::Stats => {
                if s.show_debug && !s.scoreboard {
                    telemetry(&s)
                } else {
                    String::new()
                }
            }
            Field::Help if s.mode == Mode::Deathmatch && s.show_help && !s.scoreboard => {
                "   FREE-FOR-ALL

   WASD   Move          Mouse   Aim
   LMB     Fire              R            Reload
   Space  Jump            Tab       Hold for scores
   1 Knife       2 Pistol       3 M4
   4 AK           5 .50 Sniper  6 SMG
   RMB     Sniper scope
   Esc       Game menu
   F3         Shot telemetry

   F1         Close   "
                    .into()
            }
            Field::Help => {
                if s.show_help && !s.scoreboard {
                    "   CONTROLS\n\n   WASD     Move             Mouse     Aim\n   Space     Jump             LMB         Fire\n   R             Reload          Esc           Game menu\n   1 Knife        2 Pistol        3 M4\n   4 AK            5 .50 Sniper   6 SMG\n   RMB        Sniper scope\n   E             Plant / defuse (hold) or recover\n   G             Drop bomb    B              Buy weapon kit\n   Tab         Hold for scores, tap to cycle spectator\n   F3           Shot telemetry\n   F5           Reset practice or surf\n\n   F1           Close   ".into()
                } else {
                    String::new()
                }
            }
        };
        let size = match field {
            Field::Action if value == NEARBY_FIRE => Some(14.),
            Field::Action => Some(if deathmatch { 21. } else { 16. }),
            Field::Round => Some(if deathmatch { 21. } else { 18. }),
            _ => None,
        };
        if let Some(size) = size {
            if font.font_size != size {
                font.font_size = size;
            }
        }
        if **label != value {
            **label = value;
        }
    }
}

const NEARBY_FIRE: &str = "   Nearby enemy fire   ";

fn nearby_fire_notice(session: &Session) -> &'static str {
    if session
        .nearby_fire
        .is_some_and(|(at, _)| at.elapsed() < Duration::from_millis(360))
    {
        NEARBY_FIRE
    } else {
        ""
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

fn deathmatch_notice(
    hp: u8,
    kills: u16,
    kill: bool,
    damaged: bool,
    respawned: bool,
) -> &'static str {
    if hp == 0 {
        if kill && kills == 1 {
            "   First kill  /  Respawning   "
        } else if kill {
            "   Kill confirmed  /  Respawning   "
        } else {
            "   Eliminated  /  Respawning   "
        }
    } else if kill && kills == 1 {
        "   First kill   "
    } else if kill {
        "   Kill confirmed   "
    } else if damaged {
        "   Hit taken   "
    } else if respawned {
        "   Back in play   "
    } else {
        ""
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
        if p.rifle.owns(sendit_sim::Weapon::Ak) && p.armor == sendit_sim::Loadout::Armored.armor() {
            return "   Full weapon kit equipped   ".into();
        }
        if p.money < KIT_COST {
            return format!(
                "   Full weapon kit  ${KIT_COST}   /   Need ${}   ",
                KIT_COST - p.money
            );
        }
        return format!("   B   Buy full weapon kit + 50 armor   ${KIT_COST}   ");
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deathmatch_respawn_feedback_requires_a_confirmed_new_life() {
        let mut game = sendit_sim::World::new(
            sendit_sim::Arena::load("aim").unwrap(),
            Mode::Deathmatch,
            sendit_sim::Tuning::default(),
        );
        assert!(game.add(1));
        let mut p = *game.view(1).unwrap();
        let mut notice = ReturnNotice::default();
        let now = Instant::now();
        assert!(!notice.observe(Some(&p), now));
        p.hp = 0;
        assert!(!notice.observe(Some(&p), now));
        p.hp = 100;
        p.life += 1;
        assert!(notice.observe(Some(&p), now));
        assert!(!notice.observe(Some(&p), now + Duration::from_secs(1)));
        assert!(!notice.observe(None, now + Duration::from_secs(2)));
    }
    #[test]
    fn first_kill_feedback_survives_a_trade() {
        assert_eq!(
            deathmatch_notice(0, 1, true, true, false).trim(),
            "First kill  /  Respawning"
        );
        assert_eq!(
            deathmatch_notice(100, 1, true, false, false).trim(),
            "First kill"
        );
        assert_eq!(
            deathmatch_notice(60, 0, false, true, false).trim(),
            "Hit taken"
        );
    }
}
