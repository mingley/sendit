#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//! Sendit's native first-person client.
mod audio;
mod benchmark;
mod game;
mod hud;
mod map_transfer;
mod options;
mod scene;
#[cfg(all(windows, not(debug_assertions)))]
mod windows_launch;

use bevy::{
    prelude::*,
    window::PresentMode,
    winit::{UpdateMode, WinitSettings},
};
use game::Session;
use options::Options;

fn startup_error(error: &str) -> AppExit {
    eprintln!("sendit-client: {error}");
    #[cfg(all(windows, not(debug_assertions)))]
    windows_launch::show_error(error);
    AppExit::error()
}

fn main() -> AppExit {
    #[cfg(all(windows, not(debug_assertions)))]
    windows_launch::initialize();
    let opts = match Options::parse() {
        Ok(Some(opts)) => opts,
        Ok(None) => return AppExit::Success,
        Err(error) => return startup_error(&error),
    };
    let session = match Session::open(&opts) {
        Ok(session) => session,
        Err(error) => return startup_error(&error),
    };
    let measuring = opts.benchmark.is_some();
    let local = opts.local_match;
    let mut app = App::new();
    let mut window = Window {
        title: "Sendit".into(),
        resolution: opts.window_resolution(),
        decorations: true,
        resizable: true,
        present_mode: PresentMode::AutoNoVsync,
        desired_maximum_frame_latency: std::num::NonZeroU32::new(1),
        ..default()
    };
    if opts.start_maximized {
        window.set_maximized(true);
    }
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(window),
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
    if local {
        setup_lobby_readiness(&mut app);
    }
    if measuring {
        benchmark::install(&mut app);
    }
    app.run()
}

/// The first gameplay click is accepted after a frame containing the warm arena
/// and the ready lobby UI reaches Bevy's render cleanup, after queue submission.
#[derive(Resource, Clone, Default)]
pub struct LobbyRender(std::sync::Arc<std::sync::atomic::AtomicU8>);
#[derive(Resource, Default, Clone, bevy::render::extract_resource::ExtractResource)]
struct DisplayStartPrompt(bool);

impl LobbyRender {
    pub fn click_ready(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire) >= 2
    }
}

fn setup_lobby_readiness(app: &mut App) {
    use bevy::render::{extract_resource::ExtractResourcePlugin, Render, RenderApp, RenderSet};
    let signal = LobbyRender::default();
    app.insert_resource(signal.clone())
        .init_resource::<DisplayStartPrompt>()
        .add_plugins(ExtractResourcePlugin::<DisplayStartPrompt>::default())
        .add_systems(Update, read_lobby_ready.before(game::sample));
    let render = app.sub_app_mut(RenderApp);
    render
        .insert_resource(signal)
        .add_systems(Render, finish_lobby_frame.in_set(RenderSet::Cleanup));
}

fn finish_lobby_frame(
    signal: Res<LobbyRender>,
    prompt: Option<Res<DisplayStartPrompt>>,
    cache: Res<bevy::render::render_resource::PipelineCache>,
    opaque: Res<
        bevy::render::render_phase::ViewBinnedRenderPhases<bevy::core_pipeline::core_3d::Opaque3d>,
    >,
    ui: Res<bevy::render::render_phase::ViewSortedRenderPhases<bevy::ui::TransparentUi>>,
) {
    if signal.click_ready() {
        return;
    }
    let scene_drawn = opaque.values().any(|phase| {
        phase
            .batchable_mesh_keys
            .iter()
            .chain(&phase.unbatchable_mesh_keys)
            .any(|key| cache.get_render_pipeline(key.pipeline).is_some())
    });
    let ui_drawn = ui.values().any(|phase| {
        !phase.items.is_empty()
            && phase
                .items
                .iter()
                .all(|item| cache.get_render_pipeline(item.pipeline).is_some())
    });
    if scene_drawn && ui_drawn {
        let stage = if prompt.is_some_and(|p| p.0) { 2 } else { 1 };
        signal.0.store(stage, std::sync::atomic::Ordering::Release);
    }
}

fn read_lobby_ready(
    signal: Res<LobbyRender>,
    mut prompt: ResMut<DisplayStartPrompt>,
    mut session: ResMut<Session>,
    opts: Res<Options>,
    mut announced: Local<bool>,
) {
    if *announced {
        return;
    }
    if signal.0.load(std::sync::atomic::Ordering::Acquire) >= 1 {
        session.scene_ready = true;
        prompt.0 = true;
    }
    if signal.click_ready() {
        println!(
            "QUICKPLAY_READY map=aim bots={} skill={:.2}",
            opts.bots, opts.bot_skill
        );
        *announced = true;
    }
}
