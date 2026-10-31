CREATE TABLE teams (
    name        TEXT PRIMARY KEY,
    description TEXT NOT NULL DEFAULT '',
    config      JSONB NOT NULL,
    params      JSONB NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE matches (
    id            UUID PRIMARY KEY,
    worker        TEXT NOT NULL,
    map           TEXT NOT NULL,
    -- team_a starts on CT; sides swap at halftime.
    team_a        TEXT NOT NULL REFERENCES teams(name),
    team_b        TEXT NOT NULL REFERENCES teams(name),
    -- Team configs as they were when the match was played.
    team_a_config JSONB NOT NULL,
    team_b_config JSONB NOT NULL,
    max_rounds    INT NOT NULL,
    status        TEXT NOT NULL CHECK (status IN ('running', 'finished', 'failed')),
    -- Live score and end marker, written by the api from plugin events.
    score_a       INT NOT NULL DEFAULT 0,
    score_b       INT NOT NULL DEFAULT 0,
    live_ended_at TIMESTAMPTZ,
    demo_file     TEXT NOT NULL,
    demo_status   TEXT NOT NULL DEFAULT 'pending'
                  CHECK (demo_status IN ('pending', 'processing', 'done', 'error')),
    demo_error    TEXT,
    error         TEXT,
    started_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at   TIMESTAMPTZ
);
CREATE INDEX matches_status_idx ON matches (status, worker);
CREATE INDEX matches_demo_idx ON matches (demo_status) WHERE status = 'finished';

-- Authoritative per-round results, written by the demo pipeline.
CREATE TABLE rounds (
    match_id    UUID NOT NULL REFERENCES matches(id) ON DELETE CASCADE,
    round       INT NOT NULL,
    winner_side TEXT NOT NULL CHECK (winner_side IN ('CT', 'T')),
    winner_team TEXT NOT NULL,
    reason      TEXT NOT NULL,
    end_tick    INT NOT NULL,
    PRIMARY KEY (match_id, round)
);
