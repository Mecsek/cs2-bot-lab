# BotLab CounterStrikeSharp plugin

Streams the live state of a CS2 bot-vs-bot match to the BotLab `api` over a WebSocket so
the web UI can render it in 3D. Display-only (training data comes from demos): it is
deliberately thin and never blocks the game thread.

## Versions

| Component | Version |
|---|---|
| `CounterStrikeSharp.API` NuGet | **1.0.378** (pinned in `BotLab.csproj`, latest stable as of 2026-10; `2.0.0-alpha.1` is a pre-release and not used) |
| Target framework | **net10.0** (what CSS 1.0.378 targets) |
| Build image | `mcr.microsoft.com/dotnet/sdk:10.0` |

**Metamod:Source and CounterStrikeSharp on the server must match the CS2 server build.**
Valve updates frequently break signatures/offsets; after a CS2 update, upgrade Metamod
(dev build) and CounterStrikeSharp (the `with-runtime` release, which bundles the .NET 10
runtime) together, and rebuild this plugin against the matching `CounterStrikeSharp.API`
version if the major API changes.

## Build

```sh
./build.sh          # uses podman (or docker), output in plugin/BotLab/out/
```

`out/` contains `BotLab.dll` (+ `.pdb`). The CSS API assembly is *not* copied — the server
provides it.

## Install

Copy the contents of `out/` to

```
game/csgo/addons/counterstrikesharp/plugins/BotLab/
```

on the server (i.e. `.../plugins/BotLab/BotLab.dll`). Hot reload: `css_plugins reload BotLab`.

## Configuration (environment variables of the server process)

| Variable | Default | Meaning |
|---|---|---|
| `BOTLAB_API_URL` | `ws://api:8080/ws/plugin` | WebSocket endpoint of the api (`ws://` or `wss://`) |
| `BOTLAB_SERVER_ID` | machine hostname | Identifies this server/worker in `hello` |

## What it sends

JSON text frames matching `PluginMessage` in `crates/lab-core/src/live.rs` (the source of
truth; C# mirror in `Messages.cs`):

- `{"type":"hello","server_id","map","plugin_version"}` — on every (re)connect and on map change.
- `{"type":"frame", tick, map, round, phase, round_time_left, score_ct, score_t, bomb, players}` —
  every 4 server ticks (16 Hz on 64 tick) whenever a map is loaded, including warmup and
  freezetime (`phase` = `warmup` | `freezetime` | `live` | `over`).
- `{"type":"event","tick","kind", ...}` with `kind` one of `round_start`, `freeze_end`,
  `round_end`, `kill`, `hurt`, `bomb_planted`, `bomb_defused`, `bomb_exploded`,
  `grenade_detonate`, `match_end`.

Source of each value:

- players: every controller on team 2 (T) / 3 (CT); name = `PlayerName`; `pos` = pawn
  `AbsOrigin`; `pitch`/`yaw` = `EyeAngles.X/Y`; `hp`, `ArmorValue`, helmet/defuser from pawn
  item services; `money` = `InGameMoneyServices.Account`; `equip_value` =
  `CurrentEquipmentValue`; weapons = designer names (USP-S/M4A1-S/CZ75/R8/MP5-SD corrected via
  item definition index); `flash` = remaining blind time from `BlindUntilTime` /
  `BlindStartTime + FlashDuration`.
- Dead players are still listed with `alive:false`, hp 0, empty weapons and their last known
  position (where they died).
- round = number set on `round_start` (`TotalRoundsPlayed + 1`); round clock =
  `RoundStartTime + RoundTime - CurrentTime`, clamped to `[0, RoundTime]`; scores from
  `cs_team_manager` entities; bomb from `planted_c4` (site, `C4Blow - now`, `BeingDefused`)
  or the carried/dropped `weapon_c4` (carrier position if held).
- Event weapon names are prefixed with `weapon_` (`ak47` → `weapon_ak47`); `world`, `inferno`,
  `planted_c4` and `*_projectile` pass through unchanged.
- `round_end.reason`: `target_bombed`, `bomb_defused`, `t_eliminated`, `ct_eliminated`,
  `time_ran_out`, `draw`, `t_surrender`, `ct_surrender`, `game_commencing`, ... or
  `reason_<n>` for unknown codes. Round ends without a CT/T winner (draws, warmup restarts)
  are not sent because the wire type requires a winner.
- `grenade_detonate`: `hegrenade_detonate`, `flashbang_detonate`, `smokegrenade_detonate`,
  `molotov_detonate` (molotov + incendiary), `decoy_started`.
- `match_end`: on `cs_win_panel_match`, once per map.

## Robustness

- The game thread only builds DTOs and `TryWrite`s them into bounded channels: frames
  (capacity 8, drop oldest; only the newest pending frame is sent) and events/hello
  (capacity 4096, drop oldest only on overflow).
- A background task owns the `ClientWebSocket`: serializes, sends (5 s timeout), reconnects
  with exponential backoff (1 s → 30 s) and re-sends `hello` after each reconnect.
- Every listener/event handler is wrapped in try/catch with throttled logging; entities
  are `IsValid`-checked; floats are sanitized (no NaN/Infinity on the wire).
- `Unload` completes the channels, cancels the sender and waits at most 1 s.
