use super::{amber, ash, ink, text};
use crate::game::Session;
use bevy::prelude::*;
use sendit_sim::{Mode, PlayerView};
use std::cmp::Reverse;

#[derive(Component)]
pub struct Board;
#[derive(Component)]
pub enum ScoreText {
    Header,
    Side(u8),
    Players(u8),
    Ffa(Column),
}

#[derive(Clone, Copy)]
pub enum Column {
    Rank,
    Player,
    Kills,
    Deaths,
}

#[derive(Component)]
pub enum Layout {
    Teams,
    FreeForAll,
}

fn ranked(players: &[PlayerView]) -> Vec<&PlayerView> {
    let mut ranked: Vec<_> = players.iter().collect();
    ranked.sort_unstable_by_key(|p| (Reverse(p.kills), p.deaths, p.id));
    ranked
}

fn display_name(s: &Session, p: &PlayerView) -> String {
    let raw = s.name(p.id);
    if p.id >= sendit_sim::wire::FIRST_BOT_ID {
        format!(
            "[BOT] {}",
            raw.strip_prefix("BOT ")
                .or_else(|| raw.strip_prefix("Bot "))
                .unwrap_or(&raw)
        )
    } else {
        raw
    }
}

fn spawn_ffa(panel: &mut ChildBuilder) {
    panel
        .spawn((
            Layout::FreeForAll,
            Node {
                display: Display::None,
                column_gap: Val::Px(20.),
                ..default()
            },
        ))
        .with_children(|columns| {
            for (heading, column, width) in [
                ("RANK", Column::Rank, 45.),
                ("PLAYER", Column::Player, 266.),
                ("KILLS", Column::Kills, 48.),
                ("DEATHS", Column::Deaths, 55.),
            ] {
                columns
                    .spawn(Node {
                        min_width: Val::Px(width),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(9.),
                        ..default()
                    })
                    .with_children(|col| {
                        col.spawn(text(heading, 13., ash()));
                        col.spawn((text("", 16., Color::WHITE), ScoreText::Ffa(column)));
                    });
            }
        });
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
                        .spawn((
                            Layout::Teams,
                            Node {
                                column_gap: Val::Px(26.),
                                ..default()
                            },
                        ))
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
                    spawn_ffa(panel);
                });
        });
}

pub fn update(
    s: Res<Session>,
    mut boards: Query<&mut Node, With<Board>>,
    mut layouts: Query<(&Layout, &mut Node), Without<Board>>,
    mut labels: Query<(&ScoreText, &mut Text, &mut TextFont)>,
) {
    let Ok(mut board) = boards.get_single_mut() else {
        return;
    };
    let display = if s.scoreboard && !s.local_lobby {
        Display::Flex
    } else {
        Display::None
    };
    if board.display != display {
        board.display = display;
    }
    if display == Display::None {
        return;
    }
    let ffa = s.mode == Mode::Deathmatch;
    let top = Val::Percent(if ffa && s.players.len() > 20 { 6. } else { 13. });
    if board.top != top {
        board.top = top;
    }
    for (layout, mut node) in &mut layouts {
        let visible = matches!(layout, Layout::FreeForAll) == ffa;
        let display = if visible {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
    let ranked = if ffa { ranked(&s.players) } else { Vec::new() };
    let size = if ranked.len() > 24 {
        11.
    } else if ranked.len() > 16 {
        13.
    } else {
        16.
    };
    for (kind, mut label, mut font) in &mut labels {
        let value = match *kind {
            ScoreText::Header => {
                let map = s.map.name.to_ascii_uppercase();
                if let Some(round) = s.round.filter(|_| s.mode == Mode::Competitive) {
                    format!("{map}   /   ROUND {:02}", round.number)
                } else if ffa {
                    format!("{map}   /   FREE-FOR-ALL")
                } else {
                    format!("{map}   /   SCOREBOARD")
                }
            }
            ScoreText::Ffa(column) => {
                if !ffa {
                    continue;
                }
                if font.font_size != size {
                    font.font_size = size;
                }
                ranked
                    .iter()
                    .enumerate()
                    .map(|(i, p)| match column {
                        Column::Rank => (i + 1).to_string(),
                        Column::Player => {
                            let name: String = display_name(&s, p).chars().take(23).collect();
                            if s.id == Some(p.id) {
                                format!("{name}  (you)")
                            } else {
                                name
                            }
                        }
                        Column::Kills => p.kills.to_string(),
                        Column::Deaths => p.deaths.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(
                        "
",
                    )
            }
            ScoreText::Side(_) | ScoreText::Players(_) if ffa => continue,
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
                        let name: String = display_name(&s, p).chars().take(18).collect();
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

#[cfg(test)]
mod tests {
    use super::*;
    use sendit_sim::{Arena, Tuning, World as Match};
    #[test]
    fn free_for_all_ranks_every_participant_by_kills_then_deaths() {
        let mut game = Match::new(
            Arena::load("aim").unwrap(),
            Mode::Deathmatch,
            Tuning::default(),
        );
        for (id, team) in [(1, 0), (2, 1), (3, 0), (4, 1)] {
            assert!(game.add_team(id, team));
        }
        let mut players = game.views();
        for p in &mut players {
            (p.kills, p.deaths) = match p.id {
                1 => (3, 2),
                2 => (8, 4),
                3 | 4 => (3, 1),
                _ => unreachable!(),
            };
        }
        assert_eq!(
            ranked(&players).iter().map(|p| p.id).collect::<Vec<_>>(),
            vec![2, 3, 4, 1]
        );
    }
}
