//! Sendit's native first-person client.
mod audio;
mod benchmark;
mod game;
mod hud;
mod options;
mod scene;

use bevy::{
    prelude::*,
    window::PresentMode,
    winit::{UpdateMode, WinitSettings},
};
use game::Session;
use options::Options;

fn main() -> AppExit {
    let opts = match Options::parse() {
        Ok(Some(opts)) => opts,
        Ok(None) => return AppExit::Success,
        Err(error) => {
            eprintln!("sendit-client: {error}");
            return AppExit::error();
        }
    };
    let session = match Session::open(&opts) {
        Ok(session) => session,
        Err(error) => {
            eprintln!("sendit-client: {error}");
            return AppExit::error();
        }
    };
    let measuring = opts.benchmark.is_some();
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Sendit".into(),
            resolution: (opts.width as f32, opts.height as f32).into(),
            present_mode: PresentMode::AutoNoVsync,
            desired_maximum_frame_latency: std::num::NonZeroU32::new(1),
            ..default()
        }),
        ..default()
    }))
    .insert_resource(if measuring {
        WinitSettings {
            focused_mode: UpdateMode::Continuous,
            unfocused_mode: UpdateMode::Continuous,
        }
    } else {
        WinitSettings::game()
    })
    .insert_resource(ClearColor(Color::srgb_u8(153, 178, 187)))
    .insert_resource(AmbientLight {
        color: Color::WHITE,
        brightness: 340.,
        ..default()
    })
    .insert_resource(audio::Sound::new(!opts.mute && !measuring))
    .insert_resource(opts)
    .insert_resource(session)
    .add_systems(Startup, (scene::setup, hud::setup, audio::setup))
    .add_systems(
        Update,
        (
            game::receive,
            game::sample,
            game::predict,
            scene::sync_map,
            scene::sync_players,
            scene::place_camera,
            hud::scale,
            hud::update,
            hud::scoreboard::update,
            audio::play,
        )
            .chain(),
    )
    .add_systems(Last, game::on_exit);
    if measuring {
        benchmark::install(&mut app);
    }
    app.run()
}
