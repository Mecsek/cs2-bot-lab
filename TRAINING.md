# Training on the dataset

The model is not part of this repo. This is how to train one on what `demo-pipeline` produces.

## The training table

`data/parquet/round_state_samples/map=<map>/<match_id>.parquet`. There is one file per match. Read them all
together with a glob: there is no need to merge them.

One row is one moment of a round, sampled every 0.5 s of live time (freeze time excluded).

| Columns | Use |
|---|---|
| `match_id`, `round`, `tick`, `team_ct`, `team_t` | Identifiers. Use them for splitting and grouping, not as features |
| `ct_won` (bool) | The label: did CT win this round |
| 76 Float32 feature columns | The model inputs |

The feature order comes from `crates/features/src/lib.rs::feature_names()`. To print it, run
`cargo run -p demo-pipeline -- feature-names`. The features fall into these groups:

- **Global (13):** `round_num`, `is_pistol_round`, `score_ct`, `score_t`, `round_time_left`, bomb
  state (`bomb_planted`, `bomb_site_a/b`, `bomb_time_left`, `bomb_defusing`), `alive_diff`,
  `hp_diff`, `equip_diff`.
- **Per side, `ct_*` and `t_*` (20 each):** alive, hp, armor, helmets, defusers, money,
  equipment value, weapon classes, grenades, flashed, distance to sites A/B, spread.
- **Team params, `ct_team_*` and `t_team_*` (7 each):** the bot style (difficulty, skill,
  aggression, reaction time, …). This is what makes the prediction depend on the opponent.
- **Map one-hot, `map_*` (9).**

`NaN` means "not applicable" (no bomb planted, nobody alive, sites not calibrated). Use a model
that handles NaN natively (LightGBM, XGBoost), or impute deliberately.

The other tables (`rounds`, `kills`, `damage`, `grenades`, `bomb_events`, `ticks`) are there for
analysis and for engineering new features. Training does not need them.

## Before training: calibrate bomb sites

Until `configs/maps.toml` has site centres, `*_dist_a`, `*_dist_b` and the bomb-site columns are NaN. Calibrate
once there are ~20 matches per map:

```sh
cargo run -p demo-pipeline -- calibrate-sites
```

The features are computed when a demo is processed, so the demos that were already processed have to be
reprocessed to get the new values:

```sh
docker compose exec demo-pipeline demo-pipeline reprocess
```

The running pipeline picks the matches up and overwrites each match's parquet files, replay and
`rounds` rows. It takes about 3–4 s per match.

## Loading

```sh
uv run --with polars --with lightgbm --with scikit-learn python
```

```python
import polars as pl

df = pl.read_parquet("data/parquet/round_state_samples/**/*.parquet", hive_partitioning=True)
ID_COLS = ["match_id", "round", "tick", "team_ct", "team_t", "map"]  # `map` comes from the folder name
features = [c for c in df.columns if c not in ID_COLS + ["ct_won"]]
X, y, groups = df[features].to_pandas(), df["ct_won"].to_numpy(), df["match_id"].to_numpy()
```

## Splitting

Split by `match_id`, never by row. Samples 0.5 s apart in the same round are near-duplicates,
so a row-level split leaks the answer and inflates every metric.

```python
from sklearn.model_selection import GroupShuffleSplit

train_idx, test_idx = next(GroupShuffleSplit(test_size=0.15, random_state=0).split(X, y, groups))
```

Use `GroupKFold` for cross-validation. To test generalisation to unseen opponents, hold out
every match of one team pairing (`team_ct`/`team_t`).

## Training

```python
import lightgbm as lgb

model = lgb.LGBMClassifier(objective="binary", n_estimators=2000, learning_rate=0.03)
model.fit(
    X.iloc[train_idx], y[train_idx],
    eval_set=[(X.iloc[test_idx], y[test_idx])],
    callbacks=[lgb.early_stopping(100)],
)
```

Long rounds produce more samples than short ones. To make every round count equally, pass
`sample_weight` = 1 / (number of samples in that round).

## Evaluating

The output is a probability, so judge it as one:

- **Log-loss and Brier score**, not accuracy.
- **Baseline:** the CT win rate per map. The model must beat it clearly.
- **Calibration:** bin the predictions and compare each bin with its actual win rate
  (`sklearn.calibration.calibration_curve`).
- **Slices:** check post-plant, 1vX, eco vs full-buy and late-round moments, where the model
  should be confident.

## Using it live

To chart live predictions in the UI:

1. Compute `features::extract` on the live frame in `crates/api/src/live.rs::on_frame`.
2. Broadcast `UiMessage::WinProb { tick, round, p_ct }`. The UI already plots it.

The model has to run from Rust, e.g. exported to ONNX and loaded with the `ort` crate. Training
and live inference share `features::extract`, so the features match.

## Caveats

- Only bots play, so the model learns bot behaviour, not human CS2.
- A CS2 update can change the demo format or bot behaviour. Note the CS2 build (logged by each worker) a model was trained on.
