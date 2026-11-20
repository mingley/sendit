use super::{amber, ash, ink, text};
use crate::{game::Session, menu::MenuState};
use bevy::prelude::*;
use sendit_sim::{Mode, Weapon};

#[derive(Component)]
pub struct Slots;

#[derive(Component)]
pub struct Slot(Weapon);

pub fn spawn(commands: &mut Commands) {
    commands
        .spawn((
            Slots,
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                bottom: Val::Px(24.),
                width: Val::Percent(100.),
                justify_content: JustifyContent::Center,
                column_gap: Val::Px(3.),
                ..default()
            },
        ))
        .with_children(|bar| {
            for (slot, weapon) in Weapon::ALL.into_iter().enumerate() {
                let label = format!("{}  {}", slot + 1, weapon.label().to_uppercase());
                bar.spawn((
                    Slot(weapon),
                    Node {
                        align_items: AlignItems::Center,
                        padding: UiRect::axes(Val::Px(8.), Val::Px(6.)),
                        border: UiRect::bottom(Val::Px(2.)),
                        ..default()
                    },
                    BorderColor(Color::NONE),
                    BackgroundColor(ink()),
                ))
                .with_children(|cell| {
                    cell.spawn(text(&label, 15., ash()));
                });
            }
        });
}

pub fn update(
    s: Res<Session>,
    menu: Res<MenuState>,
    mut bars: Query<&mut Node, With<Slots>>,
    mut slots: Query<(&Slot, &Children, &mut BackgroundColor, &mut BorderColor)>,
    mut colors: Query<&mut TextColor>,
) {
    let visible = !s.local_lobby
        && !menu.open
        && s.mode != Mode::Surf
        && !s.scoreboard
        && !s.show_help
        && s.me().is_some_and(|p| p.hp > 0);
    if let Ok(mut bar) = bars.get_single_mut() {
        let display = if visible {
            Display::Flex
        } else {
            Display::None
        };
        if bar.display != display {
            bar.display = display;
        }
    }
    if !visible {
        return;
    }
    let rifle = s.view_rifle();
    for (Slot(weapon), children, mut background, mut border) in &mut slots {
        let (face, edge, label) = if rifle.weapon == *weapon {
            (Color::srgb_u8(81, 60, 34), amber(), amber())
        } else if rifle.owns(*weapon) {
            (ink(), Color::srgb_u8(118, 101, 70), ash())
        } else {
            (ink(), Color::NONE, Color::srgb_u8(154, 147, 129))
        };
        background.set_if_neq(BackgroundColor(face));
        border.set_if_neq(BorderColor(edge));
        for child in children.iter() {
            if let Ok(mut color) = colors.get_mut(*child) {
                if color.0 != label {
                    color.0 = label;
                }
            }
        }
    }
}
