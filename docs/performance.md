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

Record one track at the desktop or a complete set at the host. A recording replays input under the selected rules and starting conditions. It does not store a video. These commands record and time live input on an occupied host.

```sh
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
