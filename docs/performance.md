# Measuring Sendit

The desktop target is 240 FPS (4.167 ms per frame) at 1920×1080 on a Ryzen 5 7600, GeForce RTX 4060 8 GB, and 32 GB DDR5. Use the release build with the fixed competitive render path, an uncapped display mode, and an occupied arena. The dedicated target is 128 Hz for ten players, with p99 tick work below 7.8125 ms. The shipping server reports scheduling misses separately.

## Reproduce a run

```sh
cargo build --release --locked --workspace
python3 scripts/benchmark.py server --mode competitive --map relay --players 10 --seconds 20 --output results/server.json
python3 scripts/benchmark.py client --mode competitive --map relay --players 10 --seconds 30 --width 1920 --height 1080 --output results/client.json
```

The wrapper starts a fresh server on a loopback port and spawns independent UDP peers. A client run uses one desktop client and nine peers by default. The graphical sampler requires the specified population, warms up for three seconds, and records the actual window resolution, rendering adapter, CPU, map, number of players, build profile and workload. Each peer moves and fires. The headless runner selects a complete occupancy window. Round-phase counts show how many ticks were spent buying, playing, or waiting between rounds.

The desktop report includes frame intervals, p50/p95/p99, the slowest 1% expressed as FPS, main-thread and render-schedule timings, graph encoding time, and render-device timestamps where supported. Software input-to-submit measures the interval from sampling an input to the CPU submitting the rendered frame. The result excludes mouse polling, display scanout, and panel response. Measuring input-to-photon requires an external sensor.

The host measures wall-clock duration of input and network processing plus each simulation tick. CPU is the percentage of one core consumed by the host process. RSS comes from the Linux process status. Per-client uplink and downlink include UDP application payload only. The `humans` counter includes synthetic peers connected over UDP. The fast-forward harness isolates simulation and serialization from socket scheduling and reports its own scope.

## Replay the same input

Record one track at the desktop or a complete set at the host. The desktop and host record fixed-tick inputs, original player IDs, sequences, tuning, and initial player state. Offline drill replay reconstructs movement and shots. Independent match events and remote corrections require a live server. These commands record and time live input on an occupied host.

```sh
mkdir -p results
target/release/sendit-client --connect 127.0.0.1:27015 --benchmark 30 --live-input --expect-players 10 --record relay-session.jsonl --output results/live-desktop.json
python3 scripts/benchmark.py server --mode competitive --map relay --players 10 --seconds 20 --replay relay-session.jsonl
```

These commands generate a repeatable ten-track workload.

```sh
target/release/sendit-bench record-match --map relay --players 10 --ticks 4096 --out scripted-match.jsonl
target/release/sendit-bench server --map relay --mode competitive --players 10 --warmup 1024 --ticks 1024 --replay scripted-match.jsonl
python3 scripts/benchmark.py server --map relay --mode competitive --players 10 --seconds 8 --replay scripted-match.jsonl
```

The original cloud rehearsal used the same ten scripted tracks in the fast-forward and live UDP runners. The live server's 1,024-tick window contained 263 buy ticks and 761 live ticks. Median/p95/p99 tick work was 30/92/133 µs, with one scheduler miss, 0.725% of one core, 1,724 KiB RSS and 10.177/33.367 KiB/s per client up/down. The in-process run measured 7.13/10.34/21.22 µs and excludes the UDP socket and pacing loop. [Live measurements](../benchmarks/cloud/relay-replay-udp.json) · [Fixed-loop measurements](../benchmarks/cloud/relay-replay-fixed.json)

The drill's two-client regression runs with 36 ms of simulated one-way latency, up to 10 ms of jitter in each direction, and 6% packet loss. It checks prediction against acknowledged positions, a blocked shot, a moving-target hit, health, kill credit and respawn. The default tuning replay restores settled accuracy in 46 ticks on release and 9 ticks under counter-strafe. Maximum travel in the original direction is 0.922 m and 0.172 m respectively. Friction 8.0 changes those distances to 0.574 m and 0.145 m.

## Desktop input fixture and bot workloads

The small `fixtures/relay-desktop.jsonl` recording came through the native mouse and keyboard input path on Relay. X11 automation generated the movements and clicks. The fixture contains 932 fixed ticks and 20 distinct yaw values. Its source field identifies the automation. A genuine human session can use the same commands and trace format.

```sh
target/release/sendit-bench replay --file fixtures/relay-desktop.jsonl
python3 scripts/benchmark.py server --map relay --mode competitive --players 10 --seconds 12 --warmup-seconds 12 --replay fixtures/relay-desktop.jsonl --output results/replay-server.json
python3 scripts/benchmark.py server --map relay --mode competitive --players 10 --seconds 12 --warmup-seconds 12 --bots 10 --output results/bot-server.json
python3 scripts/benchmark.py server --map relay --mode competitive --players 10 --seconds 12 --warmup-seconds 12 --bots 9 --output results/mixed-server.json
target/release/sendit-bench server --map relay --mode competitive --players 10 --bots 10 --warmup 1536 --ticks 32768
```

The live bot-only case includes AI perception, route choice, inputs and the authoritative match. A bot executes inside the server process and sends no UDP traffic. The mixed run adds one actual network peer to measure the traffic a connected player receives with nine bots. The fixed-loop report also times the bot controller itself inside the measured tick.

## Cloud release measurements

These runs used gameplay commit `1ec2bc8`, the release profile, and an AMD EPYC 9V74 host with a four-core CPU quota. Each dedicated-server sample covered 1,536 consecutive occupied ticks on Relay after 12 seconds of warmup. The ten-client replay uses the supplied single desktop fixture with staggered tracks. CPU percentages use one core as 100%.

| Dedicated workload | Tick p50 / p95 / p99 | Missed deadlines | CPU | RSS | UDP up / down per connected client |
| --- | --- | ---: | ---: | ---: | ---: |
| 10 desktop-input replay peers | 0.026 / 0.095 / 0.263 ms | 0 | 0.802% | 2,016 KiB | 17.12 / 39.54 KiB/s |
| 10 server bots | 0.018 / 0.041 / 0.076 ms | 0 | 0.508% | 2,024 KiB | No network clients |
| 9 bots and 1 network peer | 0.027 / 0.072 / 0.137 ms | 1 | 0.712% | 2,036 KiB | 12.84 / 40.31 KiB/s |

No run discarded a wall-clock slot. The mixed match missed one scheduled deadline even though its p99 work duration was below budget. Across a separate 32,768-tick fast-forward run, p99 time in the ten bots’ controllers was 0.0167 ms. [Replay host](../benchmarks/cloud/relay-desktop-input-udp.json) · [Bot host](../benchmarks/cloud/relay-bot-udp.json) · [Mixed host](../benchmarks/cloud/relay-mixed-udp.json) · [Bot controller measurements](../benchmarks/cloud/relay-bot-fixed.json)

The 1920×1080 desktop run rendered Relay deathmatch with one native client and nine bots. The Vulkan adapter was Mesa llvmpipe on the CPU. With two llvmpipe threads, three seconds of warmup, and 20 seconds of sampling, it produced 200 frame intervals and 178 render-device timestamps. Software-rendered frame p50/p95/p99 was 94.69/131.69/144.02 ms and the 1% low was 6.41 FPS. Main-schedule CPU p50/p95/p99 was 2.12/5.74/8.06 ms and the software render graph was 90.28/130.33/141.15 ms. Sampled input to render submit was 191.14 ms median and 279.08 ms p99. [Raw desktop samples and timing boundaries](../benchmarks/cloud/relay-desktop-1080p.json)

Use this command to repeat the occupied desktop scenario on the reference PC.

```sh
python3 scripts/benchmark.py client --mode deathmatch --map relay --players 10 --bots 9 --width 1920 --height 1080 --seconds 20 --output results/relay-1080p.json
```

The RTX 4060 reference run, a genuinely human-recorded match, and physical input-to-photon latency remain unmeasured. Human playtests should cover sensitivity and counter-strafe feel, recoil recovery, footstep direction, silhouette readability, and whether both Relay sites offer competitive timings.
