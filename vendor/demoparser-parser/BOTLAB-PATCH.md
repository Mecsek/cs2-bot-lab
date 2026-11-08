# demoparser `parser` crate, patched for bots

Copy of `src/parser` from https://github.com/LaihoE/demoparser at rev
`314a253ed51488093b5f28f648694e42b561079f` (the rev pinned in `crates/demo-pipeline/Cargo.toml`),
without its 58 MB `test_demo.dem` (only its own e2e tests use it). The root `Cargo.toml` swaps it
in via `[patch."https://github.com/LaihoE/demoparser"]`.

## Why

Upstream only registers a player when its steamid is not 0. Bots have steamid 0, so in a
bots-only match no player is ever registered: the parser emits no per-tick rows and
`demo-pipeline` fails with ``demo has no `tick` column``. Upstream issue: "Bots are not resolved
in events or ticks" (opened 2026-09-26).

## What changed

Three places, all marked `BOTLAB PATCH`:

- `src/second_pass/collect_data.rs` (player registration from entities)
- `src/second_pass/game_events.rs`, `handle_player_connect` (player registration while handling events)
- `src/second_pass/game_events.rs`, `entity_id_from_userid` (resolving an event's `userid`,
  `attacker`, … to a player)

In the first two, the `steamid != Some(0)` condition is dropped, and the "remove the earlier player with
the same steamid" step (`should_remove`) is skipped for steamid 0. Otherwise every bot would
replace the previous one, since they all share steamid 0. Players stay keyed by their pawn
entity id.

In the third, upstream maps a userid to a player by steamid, so every bot resolves to the same
(or no) player and `player_death`, `player_hurt`, `bomb_planted`, … lose their player fields
(`user_name`, `user_X`, `user_which_bomb_zone`, …). For steamid 0 it now resolves by slot
instead: the controller entity index is `(userid & 0xFF) + 1`.

## Bumping demoparser

Copy `src/parser` from the new rev (without `test_demo.dem` / `Cargo.lock`), re-apply the three
changes above (search for `steamid != Some(0)` and `entity_id_from_userid`), and bump the rev in
`crates/demo-pipeline/Cargo.toml`. Drop this vendored copy once upstream registers bots itself.
