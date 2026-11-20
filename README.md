# Sendit

Sendit is an original Rust tactical first-person game for community-hosted matches. A native Bevy client and a headless dedicated server share a 128 Hz movement and rifle simulation. The first playable target is a two-player drill arena with direct UDP joining, client prediction, input replay, and a repeatable occupied-match benchmark.

The provisional client target is 240 FPS at 1920×1080 on a Ryzen 5 7600, GeForce RTX 4060 8 GB, and 32 GB of DDR5. The server target is ten occupied slots with a p99 tick below 7.8125 ms. Targets require measured results for the specified workload.
