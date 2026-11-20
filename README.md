# Sendit

Sendit is an original Rust tactical first-person game for community-hosted matches. A native Bevy client and a headless dedicated server share a 128 Hz movement and rifle simulation. The first playable target is a two-player drill arena with direct UDP joining, client prediction, input replay, and a repeatable occupied-match benchmark.

The provisional client target is 240 FPS at 1920×1080 on a Ryzen 5 7600, GeForce RTX 4060 8 GB, and 32 GB of DDR5. The server target is ten occupied slots with a p99 tick below 7.8125 ms. Targets require measured results for the specified workload.

## Build and join a drill

Install Rust with rustup. On Debian or Ubuntu, install a C toolchain, pkg-config, and ALSA development headers (`build-essential pkg-config libasound2-dev`). To run the desktop game, use an X11 or XWayland display, a Vulkan-capable graphics driver, and an audio output for positional cues. The dedicated server runs without a window or audio device.

```sh
cargo build --release --locked --workspace
target/release/sendit-server --bind 0.0.0.0:27015 --map drill --mode practice
```

In another terminal on the same machine, join with the following command. A second player can use the host's IP in place of `127.0.0.1`. Allow inbound UDP on the port you chose.

```sh
target/release/sendit-client --connect 127.0.0.1:27015 --name Pilot
```

Click to capture the mouse. Use WASD to move, Space to jump, the left mouse button to fire, and R to reload. F5 resets the practice drill, F3 shows authoritative shot diagnostics, F1 shows controls, and Esc releases the cursor. Mouse sensitivity is radians per raw device count (`--sensitivity 0.002` by default).

## Repeat a peek or a match

```sh
cargo test --release --locked --workspace
target/release/sendit-probe verify --server target/release/sendit-server
target/release/sendit-bench record-peek --out peek.jsonl
target/release/sendit-bench replay --file peek.jsonl --friction 5.2
target/release/sendit-bench replay --file peek.jsonl --friction 8
python3 scripts/benchmark.py server --players 10 --seconds 10
python3 scripts/benchmark.py client --players 10 --seconds 30 --width 1920 --height 1080
```

The UDP verifier starts two fresh servers and drives two clients per run, once locally and once with delay, jitter, and loss. The peek fixture records two actors' input tracks with the same approach to the corner, then measures distance and elapsed ticks until each actor can fire a settled first shot. `Tuning::default()` in `crates/sim/src/motion.rs` sets friction, acceleration, movement spread and recovery. `crates/sim/src/rifle.rs` holds the fixed angular recoil pattern. To try the new friction live, start the host with `--friction 8`. Connected clients receive the server's tuning.

To capture actual play, add `--record session.jsonl` to a client, or to the server to collect input from every connected player. Choose a new filename for each recording. Use `sendit-client --replay session.jsonl` to replay the first track on a fresh matching map and `python3 scripts/benchmark.py server --map drill --replay session.jsonl` for occupied server measurements. Replaying fewer recorded tracks than available slots repeats tracks. Scripted workloads have a separate source label.
