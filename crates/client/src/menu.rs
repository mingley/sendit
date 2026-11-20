//! The match continues while the pointer is available for menu controls.
use crate::{game::Session, hud};
use bevy::{
    input::keyboard::{Key, KeyboardInput},
    prelude::*,
    ui::FocusPolicy,
    window::PrimaryWindow,
};

#[derive(Resource, Default)]
pub struct MenuState {
    pub open: bool,
    /// Includes the frame that resumes play, so a menu click cannot become a shot.
    pub suppress_game_input: bool,
    selection: Choice,
    cursor: Option<Vec2>,
}

#[derive(Component, Clone, Copy, PartialEq, Eq, Default)]
pub enum Choice {
    #[default]
    Resume,
    Quit,
    LobbyQuit,
}

#[derive(Component)]
pub struct Overlay;

#[derive(Component)]
pub struct ResumeCaption;

pub fn button(parent: &mut ChildBuilder, choice: Choice, caption: &str) {
    parent
        .spawn((
            Button,
            choice,
            Node {
                min_height: Val::Px(41.),
                padding: UiRect::axes(Val::Px(15.), Val::Px(8.)),
                border: UiRect::left(Val::Px(3.)),
                align_items: AlignItems::Center,
                ..default()
            },
            BorderColor(Color::srgb_u8(132, 105, 68)),
            BackgroundColor(Color::srgb_u8(48, 44, 34)),
        ))
        .with_children(|row| {
            let mut label = row.spawn((hud::text(caption, 18., Color::WHITE), FocusPolicy::Pass));
            if choice == Choice::Resume {
                label.insert(ResumeCaption);
            }
        });
}

pub fn setup(mut commands: Commands) {
    commands
        .spawn((
            Overlay,
            Node {
                display: Display::None,
                position_type: PositionType::Absolute,
                width: Val::Percent(100.),
                height: Val::Percent(100.),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            GlobalZIndex(30),
            BackgroundColor(Color::srgba(0.05, 0.05, 0.035, 0.66)),
        ))
        .with_children(|screen| {
            screen
                .spawn((
                    Node {
                        width: Val::Px(350.),
                        max_width: Val::Percent(88.),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(10.),
                        padding: UiRect::all(Val::Px(25.)),
                        border: UiRect::top(Val::Px(3.)),
                        ..default()
                    },
                    BorderColor(hud::amber()),
                    BackgroundColor(hud::panel()),
                ))
                .with_children(|panel| {
                    panel.spawn(hud::text("Game menu", 29., hud::amber()));
                    button(panel, Choice::Resume, "Resume");
                    button(panel, Choice::Quit, "Quit to desktop");
                    panel.spawn(hud::text(
                        "Up/Down select    Enter confirm\nIn match  F1 controls  Tab scores",
                        14.,
                        hud::ash(),
                    ));
                });
        });
}

pub fn input(
    windows: Query<&Window, With<PrimaryWindow>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut events: EventReader<KeyboardInput>,
    choices: Query<(&Choice, &Interaction)>,
    mut menu: ResMut<MenuState>,
    mut session: ResMut<Session>,
    mut exit: EventWriter<AppExit>,
) {
    if menu.suppress_game_input {
        menu.suppress_game_input = false;
    }
    let mut arrow = false;
    for key in events.read() {
        if key.state.is_pressed()
            && !key.repeat
            && matches!(key.logical_key, Key::ArrowUp | Key::ArrowDown)
        {
            arrow = true;
        }
    }
    let Ok(window) = windows.get_single() else {
        return;
    };
    if !window.focused {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        menu.open = !menu.open;
        menu.suppress_game_input = true;
        menu.selection = Choice::Resume;
        menu.cursor = None;
        session.captured = !menu.open && !session.local_lobby;
        session.scoreboard = false;
        return;
    }
    if !menu.open && !session.local_lobby {
        return;
    }

    // Unlocking the pointer can place it over Quit. A stationary pointer must
    // not replace the initial Resume selection before the user presses Enter.
    let cursor = window.cursor_position();
    let moved = menu.open && cursor.is_some() && menu.cursor.is_some() && cursor != menu.cursor;
    if menu.open && menu.cursor != cursor {
        menu.cursor = cursor;
    }
    let mut activate = None;
    for (choice, interaction) in &choices {
        let available = match choice {
            Choice::LobbyQuit => !menu.open,
            Choice::Resume | Choice::Quit => menu.open,
        };
        if !available {
            continue;
        }
        if *interaction == Interaction::Pressed && mouse.just_pressed(MouseButton::Left) {
            activate = Some(*choice);
        } else if moved && *interaction == Interaction::Hovered {
            menu.selection = *choice;
        }
    }
    if menu.open {
        if arrow
            || [
                KeyCode::ArrowUp,
                KeyCode::ArrowDown,
                KeyCode::KeyW,
                KeyCode::KeyS,
                KeyCode::Tab,
            ]
            .into_iter()
            .any(|key| keys.just_pressed(key))
        {
            menu.selection = if menu.selection == Choice::Resume {
                Choice::Quit
            } else {
                Choice::Resume
            };
        }
        if [KeyCode::Enter, KeyCode::NumpadEnter, KeyCode::Space]
            .into_iter()
            .any(|key| keys.just_pressed(key))
        {
            activate = Some(menu.selection);
        }
    } else if keys.just_pressed(KeyCode::KeyQ) {
        activate = Some(Choice::LobbyQuit);
    }
    if let Some(choice) = activate {
        menu.suppress_game_input = true;
        match choice {
            Choice::Resume => {
                menu.open = false;
                session.captured = !session.local_lobby;
            }
            Choice::Quit | Choice::LobbyQuit => {
                session.captured = false;
                exit.send(AppExit::Success);
            }
        }
    }
}

pub fn paint(
    menu: Res<MenuState>,
    session: Res<Session>,
    mut overlay: Query<&mut Node, With<Overlay>>,
    mut choices: Query<(
        &Choice,
        &Interaction,
        &mut BackgroundColor,
        &mut BorderColor,
    )>,
    mut resume: Query<&mut Text, With<ResumeCaption>>,
) {
    if let Ok(mut root) = overlay.get_single_mut() {
        let display = if menu.open {
            Display::Flex
        } else {
            Display::None
        };
        if root.display != display {
            root.display = display;
        }
    }
    if !menu.open && !session.local_lobby {
        return;
    }
    if let Ok(mut caption) = resume.get_single_mut() {
        let label = if session.local_lobby {
            "Back"
        } else {
            "Resume"
        };
        if caption.as_str() != label {
            **caption = label.to_string();
        }
    }
    for (choice, interaction, mut background, mut border) in &mut choices {
        let selected = if *choice == Choice::LobbyQuit {
            *interaction != Interaction::None && !menu.open
        } else {
            menu.selection == *choice && menu.open
        };
        let (face, edge) = if selected {
            (Color::srgb_u8(100, 69, 36), hud::amber())
        } else {
            (Color::srgb_u8(48, 44, 34), Color::srgb_u8(132, 105, 68))
        };
        background.set_if_neq(BackgroundColor(face));
        border.set_if_neq(BorderColor(edge));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{game, options::Options};
    use bevy::{input::mouse::MouseMotion, window::CursorGrabMode};

    fn fixture(lobby: bool) -> (App, Entity) {
        let opts = Options {
            local_match: lobby,
            ..default()
        };
        let mut session = Session::open(&opts).unwrap();
        session.captured = !lobby;
        let mut app = App::new();
        app.insert_resource(opts)
            .insert_resource(session)
            .init_resource::<MenuState>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_event::<KeyboardInput>()
            .add_event::<MouseMotion>()
            .add_event::<AppExit>()
            .add_systems(Update, (input, game::sample).chain());
        let window = app
            .world_mut()
            .spawn((
                Window {
                    focused: true,
                    ..default()
                },
                PrimaryWindow,
            ))
            .id();
        (app, window)
    }

    // Mirror InputPlugin's frame boundary while retaining physically held inputs.
    fn advance(app: &mut App) {
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
    }

    #[test]
    fn resume_click_closes_the_menu_without_firing_even_when_held() {
        let (mut app, window) = fixture(false);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        advance(&mut app);
        assert!(app.world().resource::<MenuState>().open);
        let cursor = &app.world().get::<Window>(window).unwrap().cursor_options;
        assert!(cursor.visible);
        assert_eq!(cursor.grab_mode, CursorGrabMode::None);

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        advance(&mut app);
        assert_eq!(app.world().resource::<Session>().input.forward, 0.);

        app.world_mut()
            .spawn((Choice::Resume, Interaction::Pressed));
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        advance(&mut app);
        assert!(!app.world().resource::<MenuState>().open);
        assert!(!app.world().resource::<Session>().input.fire);
        assert!(
            !app.world()
                .get::<Window>(window)
                .unwrap()
                .cursor_options
                .visible
        );
        advance(&mut app);
        assert!(!app.world().resource::<Session>().input.fire);

        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Left);
        advance(&mut app);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        advance(&mut app);
        assert!(app.world().resource::<Session>().input.fire);
    }

    #[test]
    fn match_keyboard_and_welcome_button_both_request_a_real_exit() {
        let (mut app, _) = fixture(false);
        for key in [KeyCode::Escape, KeyCode::ArrowDown, KeyCode::Enter] {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(key);
            advance(&mut app);
        }
        assert!(matches!(app.should_exit(), Some(AppExit::Success)));

        let (mut lobby, _) = fixture(true);
        lobby
            .world_mut()
            .spawn((Choice::LobbyQuit, Interaction::Pressed));
        lobby
            .world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        advance(&mut lobby);
        assert!(matches!(lobby.should_exit(), Some(AppExit::Success)));
        assert!(lobby.world().resource::<Session>().local_lobby);
    }

    #[test]
    fn opening_under_a_stationary_pointer_keeps_resume_selected() {
        let (mut app, window) = fixture(false);
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .set_cursor_position(Some(Vec2::new(180., 100.)));
        app.world_mut().spawn((Choice::Quit, Interaction::Hovered));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        advance(&mut app);
        advance(&mut app);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Enter);
        advance(&mut app);
        assert!(!app.world().resource::<MenuState>().open);
        assert!(app.world().resource::<Session>().captured);
        assert!(app.should_exit().is_none());
    }
}
