use super::{amber, ash, ink, text};
use crate::game::Session;
use bevy::prelude::*;
use sendit_sim::Mode;

#[derive(Component)]
pub struct Board;
#[derive(Component)]
pub enum ScoreText {
    Header,
    Side(u8),
    Players(u8),
}

pub fn spawn(commands: &mut Commands) {
    commands
        .spawn((
            Board,
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                top: Val::Percent(13.),
                width: Val::Percent(100.),
                justify_content: JustifyContent::Center,
                ..default()
            },
            ZIndex(10),
        ))
        .with_children(|screen| {
            screen
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(12.),
                        padding: UiRect::all(Val::Px(20.)),
                        min_width: Val::Px(550.),
                        ..default()
                    },
                    BackgroundColor(ink()),
                ))
                .with_children(|panel| {
                    panel.spawn((text("", 19., Color::WHITE), ScoreText::Header));
                    panel
                        .spawn(Node {
                            column_gap: Val::Px(26.),
                            ..default()
                        })
                        .with_children(|columns| {
                            for team in [0, 1] {
                                let accent = if team == 0 {
                                    amber()
                                } else {
                                    Color::srgb_u8(129, 203, 238)
                                };
                                columns
                                    .spawn(Node {
                                        flex_direction: FlexDirection::Column,
                                        row_gap: Val::Px(8.),
                                        min_width: Val::Px(228.),
                                        ..default()
                                    })
                                    .with_children(|side| {
                                        side.spawn((text("", 16., accent), ScoreText::Side(team)));
                                        side.spawn(text(
                                            "    PLAYER               K   D",
                                            12.,
                                            ash(),
                                        ));
                                        side.spawn((
                                            text("", 13., Color::WHITE),
                                            ScoreText::Players(team),
                                        ));
                                    });
                            }
                        });
                });
        });
}

pub fn update(
    s: Res<Session>,
    mut boards: Query<&mut Node, With<Board>>,
    mut labels: Query<(&ScoreText, &mut Text)>,
) {
    let Ok(mut board) = boards.get_single_mut() else {
        return;
    };
    let display = if s.scoreboard {
        Display::Flex
    } else {
        Display::None
    };
    if board.display != display {
        board.display = display;
    }
    if !s.scoreboard {
        return;
    }
    for (kind, mut label) in &mut labels {
        let value = match *kind {
            ScoreText::Header => {
                let map = s.map.name.to_ascii_uppercase();
                if let Some(round) = s.round.filter(|_| s.mode == Mode::Competitive) {
                    format!("{map}   /   ROUND {:02}", round.number)
                } else {
                    format!("{map}   /   SCOREBOARD")
                }
            }
            ScoreText::Side(team) => {
                let heading = if team == 0 {
                    "TERRORISTS"
                } else {
                    "COUNTER-TERRORISTS"
                };
                let alive = s
                    .players
                    .iter()
                    .filter(|p| p.team == team && p.hp > 0)
                    .count();
                if let Some(round) = s.round.filter(|_| s.mode == Mode::Competitive) {
                    format!(
                        "{heading}  {}  |  {alive} alive",
                        round.score[team as usize]
                    )
                } else {
                    heading.into()
                }
            }
            ScoreText::Players(team) => {
                let mut players: Vec<_> = s.players.iter().filter(|p| p.team == team).collect();
                players.sort_by_key(|p| (std::cmp::Reverse(p.kills), p.deaths, p.id));
                if players.is_empty() {
                    "    Waiting for players".into()
                } else {
                    let mut rows = Vec::with_capacity(players.len());
                    for p in players {
                        let marker = if Some(p.id) == s.id { '>' } else { ' ' };
                        let state = if p.hp == 0 { 'x' } else { ' ' };
                        let name: String = s.name(p.id).chars().take(18).collect();
                        rows.push(format!(
                            "{marker}{state}  {name:<18} {:>3} {:>3}",
                            p.kills, p.deaths
                        ));
                    }
                    rows.join("\n")
                }
            }
        };
        if **label != value {
            **label = value;
        }
    }
}
