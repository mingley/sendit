# Maps and custom arenas

The server chooses the map and ruleset independently. These original arenas ship inside both binaries.

| Map ID | Layout | Example session |
| --- | --- | --- |
| `drill` | Straight 16 m rifle lane, marked target plates, offset peek walls, eight 18 cm steps and a split surf bank | Practice and input replay |
| `relay` | Two painted sites with outer approaches, a central gantry and a rear defender connector | Competitive 5v5 |
| `aim` | Staggered cover and a single central objective, with five spawn positions per team | Deathmatch or a two-player competitive duel |
| `surf` | Elevated start, split entry bank, two gaps, alternate side banks and a striped finish gate | Surf |

```sh
target/release/sendit-server --map aim --mode deathmatch
target/release/sendit-server --map aim --mode competitive --max-players 2
target/release/sendit-server --map surf --mode surf
```

Each command starts a separate session. On Surf, face the first stripe and hold forward from the start pad to ride the center line. The first seam is 8 m past the start stripe. A pair of red marker posts signals the first gap. Steer onto an outer bank to vary the second transfer. The course finishes beneath the arch. Press F5 to return to the start.

## Install a map

Place a JSON file in `maps/` under the server's working directory, or point `SENDIT_MAP_DIR` at a directory you manage. Use a filename stem of up to 48 ASCII letters, digits, hyphens, or underscores. Give the map the same identifier in its `name` field. Choose a fresh name when adapting an embedded arena.

For example, this command creates `community_lane.json` from Aim in a separate directory, then hosts it with the release server.

```sh
mkdir -p /tmp/sendit-community-maps
python3 - <<'PY'
import json
from pathlib import Path

arena = json.loads(Path("maps/aim.json").read_text())
arena["name"] = "community_lane"
Path("/tmp/sendit-community-maps/community_lane.json").write_text(json.dumps(arena) + "\n")
PY
SENDIT_MAP_DIR=/tmp/sendit-community-maps target/release/sendit-server --map community_lane --mode deathmatch
```

Joining clients request the installed map data from the server. Both endpoints validate the same geometry before the client uses it for prediction. Reload the map on the host after editing it. Map changes require no Rust build.

## Geometry and conventions

Positions are `[x, y, z]` in meters. Y points up. Spawn and navigation positions identify the player's feet. A spawn yaw of zero faces −Z. Positive yaw in radians turns right. Lay out an explicit floor wherever a player should be able to stand.

| Field | Meaning |
| --- | --- |
| `name` | Map identifier matching the filename |
| `boxes` | Axis-aligned solids with `min`, `max`, and `material` |
| `ramps` | Wedges with the same bounds plus `axis` set to `"x"` or `"z"`, and a Boolean `rising` |
| `spawns` | Objects with `pos`, `yaw`, and `team`. Team 0 starts on the attacking side and team 1 on the defending side |
| `sites` | Objectives, each with a short `name`, a center `pos`, and a `radius` from 0.25 through 32 m |
| `nav` | Traversable landmarks at junctions, cover corners, approach lanes and objectives for bots |
| `course` | Ordered centers of surf gates at surface height. The first and last entries mark the start and finish |

`ramps`, `sites`, `nav`, and `course` may be omitted. The supported materials are `floor`, `wall`, `cover`, `trim`, `target`, `ramp`, `step`, `site_a`, and `site_b`. Material selects the surface appearance. The box or wedge shape supplies collision and shot occlusion.

For a rising X ramp, the top runs from `min.y` at `min.x` to `max.y` at `max.x`. Setting `rising` to false reverses the slope. A Z ramp uses the same convention along Z. A surf seam can share two endpoints and a gradient with its neighbor. Inspect the first two wedges in `surf.json` for a playable example. [Surf course data](../maps/surf.json)

The loader accepts up to 48 KiB of JSON, 512 boxes and ramps combined, 64 spawns, 8 sites and 256 navigation points. A marked course has 2 through 64 points. Geometry must have positive extent of at least 1 mm on each axis and sit within ±2048 m. Give each team clear spawns, leave at least 0.30 m of horizontal clearance around navigation points and allow 1.8 m of standing headroom.

## Verify an edit

Join the host and walk the routes in both directions. Check plant and recovery positions in competitive mode, sightlines at standing eye height and movement at adjoining surfaces. The repository tests also drive the authored routes and exercise exact ray intersections.

```sh
cargo test --release --locked -p sendit-sim map::tests
```
