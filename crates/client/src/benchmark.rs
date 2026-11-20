//! Occupied-match instrumentation, installed only for an explicit benchmark.
use crate::{game::Session, options::Options};
use bevy::{
    diagnostic::{DiagnosticPath, DiagnosticsStore},
    prelude::*,
    render::{
        diagnostic::{RecordDiagnostics, RenderDiagnosticsPlugin},
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        graph::CameraDriverLabel,
        render_graph::{Node, NodeRunError, RenderGraph, RenderGraphContext, RenderLabel},
        renderer::{RenderAdapterInfo, RenderContext},
        Render, RenderApp, RenderSet,
    },
    window::PrimaryWindow,
};
use serde_json::{json, Value};
use std::{
    fs,
    sync::{mpsc, Mutex},
    time::{Duration, Instant},
};

const CPU_PATH: DiagnosticPath = DiagnosticPath::const_new("render/sendit_frame/elapsed_cpu");
const GPU_PATH: DiagnosticPath = DiagnosticPath::const_new("render/sendit_frame/elapsed_gpu");
const WARMUP: Duration = Duration::from_secs(3);

#[derive(Resource, Default, Clone, ExtractResource)]
pub struct InputClock(pub Option<Instant>);

#[derive(Resource, Default, Clone, ExtractResource)]
struct Active(bool);

struct Completion {
    render_ms: f64,
    input_ms: Option<f64>,
}

#[derive(Resource)]
struct Outbox(mpsc::SyncSender<Completion>);

#[derive(Resource)]
struct Inbox(Mutex<mpsc::Receiver<Completion>>);

#[derive(RenderLabel, Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Boundary {
    Begin,
    End,
}

impl Node for Boundary {
    fn run(
        &self,
        _: &mut RenderGraphContext,
        context: &mut RenderContext,
        world: &World,
    ) -> Result<(), NodeRunError> {
        if world.get_resource::<Active>().is_some_and(|state| state.0) {
            let recorder = context.diagnostic_recorder();
            match self {
                Self::Begin => {
                    recorder.begin_time_span(context.command_encoder(), "sendit_frame".into())
                }
                Self::End => recorder.end_time_span(context.command_encoder()),
            }
        }
        Ok(())
    }
}

struct BenchmarkPlugin;
impl Plugin for BenchmarkPlugin {
    fn build(&self, app: &mut App) {
        let (tx, rx) = mpsc::sync_channel(256);
        app.init_resource::<Book>()
            .init_resource::<InputClock>()
            .init_resource::<Active>()
            .insert_resource(Inbox(Mutex::new(rx)))
            .add_systems(First, start_main_frame)
            .add_systems(Last, sample);
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render
                .insert_resource(Outbox(tx))
                .init_resource::<RenderStart>()
                .add_systems(Render, start_render.before(RenderSet::ExtractCommands))
                .add_systems(Render, finish_render.in_set(RenderSet::Cleanup));
        }
    }

    fn finish(&self, app: &mut App) {
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            let mut graph = render.world_mut().resource_mut::<RenderGraph>();
            // PBR installs its optional preprocessing node during plugin finish.
            // Bracket every top-level root before the whole camera graph executes.
            let roots: Vec<_> = graph
                .iter_nodes()
                .filter(|n| n.edges.input_edges().is_empty())
                .map(|n| n.label)
                .collect();
            graph.add_node(Boundary::Begin, Boundary::Begin);
            graph.add_node(Boundary::End, Boundary::End);
            for root in roots {
                graph.add_node_edge(Boundary::Begin, root);
            }
            graph.add_node_edge(CameraDriverLabel, Boundary::End);
        }
    }
}

pub fn install(app: &mut App) {
    app.add_plugins((
        RenderDiagnosticsPlugin,
        ExtractResourcePlugin::<Active>::default(),
        ExtractResourcePlugin::<InputClock>::default(),
        BenchmarkPlugin,
    ));
}

#[derive(Resource, Default)]
struct RenderStart(Option<Instant>);

fn start_render(mut start: ResMut<RenderStart>) {
    start.0 = Some(Instant::now());
}

fn finish_render(
    active: Option<Res<Active>>,
    start: Res<RenderStart>,
    clock: Option<Res<InputClock>>,
    out: Res<Outbox>,
    mut previous: Local<Option<Instant>>,
) {
    // Count a sampled input once, on the first rendered frame which contains it.
    let input = clock
        .and_then(|c| c.0)
        .filter(|stamp| Some(*stamp) != *previous);
    if input.is_some() {
        *previous = input;
    }
    if !active.as_deref().is_some_and(|a| a.0) {
        return;
    }
    let Some(start) = start.0 else { return };
    let now = Instant::now();
    let _ = out.0.try_send(Completion {
        render_ms: ms(now - start),
        input_ms: input.map(|stamp| ms(now - stamp)),
    });
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[derive(Resource)]
struct Book {
    boot: Instant,
    frame: Instant,
    previous: Option<Instant>,
    last_server_tick: u32,
    last_snapshot: Instant,
    warm: Option<Instant>,
    start: Option<Instant>,
    drain: Option<Instant>,
    drain_frames: u32,
    done: bool,
    players_min: usize,
    players_max: usize,
    map_revision: u64,
    map: String,
    cpu_main: Vec<f64>,
    wall: Vec<f64>,
    render: Vec<f64>,
    input: Vec<f64>,
    graph: Vec<f64>,
    gpu: Vec<f64>,
    gpu_stamp: Option<bevy::utils::Instant>,
    graph_stamp: Option<bevy::utils::Instant>,
}

impl Default for Book {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            boot: now,
            frame: now,
            previous: None,
            last_server_tick: 0,
            last_snapshot: now,
            warm: None,
            start: None,
            drain: None,
            drain_frames: 0,
            done: false,
            players_min: usize::MAX,
            players_max: 0,
            map_revision: 0,
            map: String::new(),
            cpu_main: Vec::new(),
            wall: Vec::new(),
            render: Vec::new(),
            input: Vec::new(),
            graph: Vec::new(),
            gpu: Vec::new(),
            gpu_stamp: None,
            graph_stamp: None,
        }
    }
}

fn start_main_frame(mut book: ResMut<Book>) {
    book.frame = Instant::now();
}

fn collect(
    store: &DiagnosticsStore,
    path: &DiagnosticPath,
    last: &mut Option<bevy::utils::Instant>,
    out: &mut Vec<f64>,
) {
    if let Some(measurement) = store.get(path).and_then(|d| d.measurement()) {
        if Some(measurement.time) != *last {
            *last = Some(measurement.time);
            if measurement.value.is_finite() && measurement.value >= 0.0 {
                out.push(measurement.value);
            }
        }
    }
}

fn stats(values: &[f64]) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let p = |rank: usize| sorted[(sorted.len() * rank).div_ceil(100) - 1];
    json!({"samples":sorted.len(),"p50":p(50),"p95":p(95),"p99":p(99),"max":sorted.last()})
}

fn low_one_percent(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| b.total_cmp(a));
    let n = sorted.len().div_ceil(100);
    let total: f64 = sorted[..n].iter().sum();
    (total > 0.).then_some(1000. * n as f64 / total)
}

fn fail(book: &mut Book, exit: &mut EventWriter<AppExit>, message: &str) {
    book.done = true;
    eprintln!("sendit-client benchmark: {message}");
    exit.send(AppExit::error());
}

#[allow(clippy::too_many_arguments)]
fn sample(
    mut state: ResMut<Book>,
    mut active: ResMut<Active>,
    session: Res<Session>,
    opts: Res<Options>,
    mut inbox: ResMut<Inbox>,
    diagnostics: Res<DiagnosticsStore>,
    adapter: Option<Res<RenderAdapterInfo>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut exit: EventWriter<AppExit>,
) {
    let now = Instant::now();
    let book = &mut *state;
    if book.done {
        return;
    }
    let seconds = opts.benchmark.expect("installed only for a benchmark");
    if let Ok(rx) = inbox.0.get_mut() {
        for completion in rx.try_iter() {
            book.render.push(completion.render_ms);
            if let Some(latency) = completion.input_ms {
                book.input.push(latency);
            }
        }
    }
    collect(&diagnostics, &GPU_PATH, &mut book.gpu_stamp, &mut book.gpu);
    collect(
        &diagnostics,
        &CPU_PATH,
        &mut book.graph_stamp,
        &mut book.graph,
    );
    if session.server_tick != book.last_server_tick {
        book.last_server_tick = session.server_tick;
        book.last_snapshot = now;
    }
    let count = session.players.len();
    let ready = session.me().is_some()
        && count >= opts.expect_players
        && now - book.last_snapshot < Duration::from_secs(2);
    if let Some(drain) = book.drain {
        book.drain_frames += 1;
        if book.drain_frames < 8 && now - drain < Duration::from_secs(2) {
            return;
        }
        let Ok(window) = windows.get_single() else {
            fail(book, &mut exit, "no primary window");
            return;
        };
        let cpu = fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|info| {
                info.lines().find_map(|l| {
                    l.strip_prefix("model name")
                        .and_then(|tail| tail.split_once(':'))
                        .map(|(_, v)| v.trim().to_owned())
                })
            })
            .or_else(|| std::env::var("PROCESSOR_IDENTIFIER").ok());
        let gpu_info=adapter.as_deref().map(|a|json!({
            "name":a.name,"type":format!("{:?}",a.device_type),"backend":format!("{:?}",a.backend),
            "driver":a.driver,"driver_info":a.driver_info,"vendor":a.vendor,"device":a.device
        }));
        let report = json!({
            "benchmark":"occupied-desktop","build_profile":option_env!("SENDIT_PROFILE"),"target":option_env!("SENDIT_TARGET"),
            "cpu":cpu,"gpu":gpu_info,"os":std::env::consts::OS,"resolution_px":[window.physical_width(),window.physical_height()],
            "present_mode":format!("{:?}",window.present_mode),
            "mode":format!("{:?}",session.mode),"map":book.map,"players_min":book.players_min,"players_max":book.players_max,
            "expected_players":opts.expect_players,"warmup_seconds":WARMUP.as_secs(),"measured_seconds":(drain-book.start.unwrap()).as_secs_f64(),
            "workload":session.replay.as_ref().map_or(if opts.live_input {"live-device-input"} else {"deterministic-client-input"},|r|r.header.source.as_str()),
            "frame_interval_ms":stats(&book.wall), "one_percent_low_fps":low_one_percent(&book.wall),
            "one_percent_low_definition":"1000 divided by the mean duration in ms of the slowest ceil(frame count / 100) intervals",
            "cpu_main_schedule_ms":stats(&book.cpu_main),"cpu_render_schedule_ms":stats(&book.render),
            "cpu_encode_graph_ms":stats(&book.graph),"gpu_render_graph_ms":stats(&book.gpu),
            "gpu_sample_status":if book.gpu.is_empty(){"no hardware timestamp samples returned"}else{"timestamp query results"},
            "sampled_input_to_render_submit_ms":stats(&book.input), "input_to_photon_ms":Value::Null,
            "input_to_photon_status":"requires external sensor measurement",
            "local_input_trace":session.replay.as_ref().map(|t|
                json!({"source":t.header.source,"tracks":t.track_count(),"duration_ticks":t.duration(),"played_slot":0})),
            "timing_boundaries":{
                "frame_interval_ms":"elapsed wall time between main-world First systems",
                "cpu_main_schedule_ms":"main-world First to Last, before extraction",
                "cpu_render_schedule_ms":"render-world schedule start to cleanup immediately after submit/present calls; includes any waits",
                "cpu_encode_graph_ms":"CPU clock around top-level render graph, including camera subgraphs",
                "gpu_render_graph_ms":"GPU timestamps bracketing graph commands through the last camera, asynchronous samples",
                "sampled_input_to_render_submit_ms":"most recent sampled input in an extracted frame to first render cleanup after queue submit/present on the CPU; excludes device polling, GPU completion and scanout"
            },
            "raw_ms":{"frame_interval":book.wall,"cpu_main":book.cpu_main,"cpu_render":book.render,
                "cpu_graph":book.graph,"gpu_graph":book.gpu,"sampled_input_to_render_submit":book.input}
        });
        match serde_json::to_vec_pretty(&report)
            .map_err(std::io::Error::other)
            .and_then(|data| fs::write(&opts.output, data))
        {
            Ok(()) => {
                println!(
                    "benchmark saved {} ({} frame intervals, {} GPU timestamps, {}–{} players)",
                    opts.output.display(),
                    book.wall.len(),
                    book.gpu.len(),
                    book.players_min,
                    book.players_max
                );
                book.done = true;
                exit.send(AppExit::Success);
            }
            Err(error) => fail(
                book,
                &mut exit,
                &format!("writing {}: {error}", opts.output.display()),
            ),
        }
        return;
    }
    if let Some(start) = book.start {
        if !ready || book.map_revision != session.map_revision {
            fail(
                book,
                &mut exit,
                "occupancy, connection or map changed during the measured interval",
            );
            return;
        }
        book.players_min = book.players_min.min(count);
        book.players_max = book.players_max.max(count);
        book.cpu_main.push(ms(now - book.frame));
        if let Some(previous) = book.previous {
            book.wall.push(ms(book.frame - previous));
        }
        book.previous = Some(book.frame);
        if (now - start).as_secs_f32() >= seconds && book.wall.len() >= 2 {
            active.0 = false;
            book.drain = Some(now);
        }
        return;
    }
    if ready {
        let warm = *book.warm.get_or_insert(now);
        if now - warm >= WARMUP {
            book.start = Some(now);
            book.map_revision = session.map_revision;
            book.map = session.map.name.clone();
            active.0 = true;
        }
    } else {
        book.warm = None;
    }
    if book.start.is_none() && now - book.boot > Duration::from_secs(60) {
        fail(
            book,
            &mut exit,
            &format!(
                "no stable match with {} players ({})",
                opts.expect_players, session.status
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn low_fps_uses_the_mean_of_the_slowest_percent() {
        let mut frames = vec![2.0; 198];
        frames.extend([10., 30.]);
        assert_eq!(low_one_percent(&frames), Some(50.0));
        assert_eq!(stats(&frames)["p99"], 2.0);
        assert!(low_one_percent(&[]).is_none());
    }
}
