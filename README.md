# cs2-bot-lab

Self-running CS2 bot-vs-bot matches → a labelled dataset for **round win probability**
("given this moment of the round, how likely is CT to win against this opponent?"), plus a
live 3D viewer (separate project: `../cs2-lab-ui`).

```
cs2 worker ×4 ──(plugin WS, 16 Hz)──▶ api ──WS──▶ cs2-lab-ui (3D live + replays)
     ▲   └─(tv_record .dem)─▶ data/demos ─▶ demo-pipeline ─▶ data/parquet, data/replays
     └─RCON── orchestrator ──▶ Postgres ◀── api / demo-pipeline
```

The model itself is not part of this repo: train your own on `round_state_samples`.

| Path | What |
|---|---|
| `crates/lab-core` | Teams, botprofile.db rendering, weapon names, wire types (`live.rs`) |
| `crates/features` | Round state → 76 named features (the training table's columns) |
| `crates/orchestrator` | Random matchup sampler, one RCON driver per worker, watchdog |
| `crates/api` | Plugin ingest, UI WebSocket fan-out, REST |
| `crates/demo-pipeline` | Demo → Parquet tables + replay (demoparser Rust core) |
| `plugin/BotLab` | CounterStrikeSharp plugin streaming live state (C#) |
| `configs/teams/*.toml` | Bot team roster (16 styles) |
| `vendor/csgoproto` | demoparser's protobuf crate with protos pinned (see its `build.rs`) |

## Setup (Podman)

```sh
cp .env.example .env                          # passwords, METAMOD_URL, CSS_URL, CS2_BASE_DIR
systemctl --user enable --now podman.socket   # lets the orchestrator restart hung workers
podman compose --profile install run --rm cs2-install           # downloads CS2 once (~60 GB); rerun to resume
cargo run -p orchestrator -- check-teams
cargo run -p orchestrator -- gen-botprofile   # -> docker/worker/botprofile.db (rerun after editing teams)
plugin/BotLab/build.sh                        # -> plugin/BotLab/out/
podman compose up -d --build                  # add `--profile ui` to also serve the UI on :8081
```

### Shared CS2 install
CS2 is downloaded **once** into `CS2_BASE_DIR` (default `./cs2-base`) by the `cs2-install`
service. If the download is interrupted, run it again: SteamCMD resumes from what is already in
that directory.

The workers never run SteamCMD. Each one mounts the shared install as a throwaway Podman
overlay (`:O`). A worker sees its own writable copy (plugins, patched `gameinfo.gi`, configs,
logs), the base directory is never modified, and the worker's changes are discarded when it
stops. `worker-entry.sh` re-applies them on every start. This means:
- ~60 GB of disk in total instead of 60 GB per worker
- workers start in seconds
- a watchdog restart never updates the game

**Updating CS2** is a deliberate step. Workers that are running while the base changes
underneath their overlay could break, so stop them first:
```sh
podman compose stop worker1 worker2 worker3 worker4
podman compose --profile install run --rm cs2-install
podman compose up -d
```
Each worker logs the CS2 build it runs (`worker-entry: CS2 build …`). After a Valve update,
Metamod and CounterStrikeSharp may need newer releases (`METAMOD_URL` / `CSS_URL`) before the
plugin loads again.

### Worker image
`METAMOD_URL` / `CSS_URL` must point at the **Linux** Metamod:Source 2.x build (`.tar.gz`)
and the CounterStrikeSharp *with-runtime* Linux release (`.zip`) that support the current
CS2 build. The plugin is built against `CounterStrikeSharp.API` 1.0.378, so use that
CounterStrikeSharp release. The image only carries these addons and our configs; the game comes
from the shared install.

Workers run with `userns_mode: keep-id`, so the container's uid 1000 is your host user and
`./data/demos` and `CS2_BASE_DIR` stay owned by you. Budget about 2 cores and 4 GB of RAM per
worker.

## Workflow

- **Monitor:** `docker compose logs -f orchestrator demo-pipeline`, the UI lobby, or
  `GET /api/matches?status=running`.
- **Bomb sites:** after ~20 matches per map, run `cargo run -p demo-pipeline -- calibrate-sites`
  to fill `configs/maps.toml`. Until then the site-distance features are NaN.
- **Your model:** train on `round_state_samples` (label `ct_won`). Split by `match_id`,
  because samples within a round are near-duplicates.
  `cargo run -p demo-pipeline -- feature-names` prints the feature columns in order.
  To show live predictions in the UI, compute `features::extract` on the live frame (in
  `crates/api/src/live.rs::on_frame`) and broadcast `UiMessage::WinProb`. The UI already
  charts it.
- **Debug one demo:** `cargo run -p demo-pipeline -- process path/to/match.dem`.

## Dataset (`data/parquet/<table>/map=<map>/<match_id>.parquet`)
- **`round_state_samples`:** one row per 0.5 s of live round time: `match_id, round, tick,
  team_ct, team_t, ct_won` plus the features. This is the training table.
- **`rounds`:** start, freeze-end and end ticks, the teams on each side, the score before the
  round, the winner and the reason.
- **`ticks`:** every player every 8 ticks: position, view angles, hp, armor, money, equipment
  value and active weapon.
- **`kills`, `damage`, `grenades`, `bomb_events`.**

Matches are labelled from demos only. The live stream is only used for display and to tell the
orchestrator when a match has ended.

## Verified vs. not yet verified
**Verified locally:**
- Unit tests: `cargo test --workspace`, 17 tests.
- Clippy is clean.
- The services image builds in a container.
- An integration run against Postgres: migrations, team sync, a simulated plugin stream to the
  api, the UI WebSocket receiving frames and events, the score write and
  `live_ended_at`, and the REST endpoints.
- The plugin compiles, and every plugin message type deserializes in `lab-core`.
- The UI builds and renders against its mock.

**Needs a real server (spike checklist):**
1. Metamod and CounterStrikeSharp load on the current build, and BotLab connects
   (look for `plugin connected` in the api log).
2. A bots-only match runs with no humans, and `bot_add ct expert "RUSHERS_1"` picks up the
   custom profile from the loose `botprofile.db`. If it doesn't, profiles fall back to
   default bots and only the difficulty differs.
3. `tv_record "botlab_demos/match_…"` writes into the subdirectory.
4. demoparser parses the demo:
   - `demo-pipeline process` prints sensible rounds and scores.
   - `which_bomb_zone` 1/2 maps to A/B.
5. Plugin details, best checked by comparing the live view with the demo:
   - round-end reason codes
   - bomb-site lookup
   - `molotov_detonate` / `decoy_started` firing
   - flash timing
   - round clock during freezetime
6. A map GLB exported with Source2Viewer lines up in the UI (see `../cs2-lab-ui/README.md`).
