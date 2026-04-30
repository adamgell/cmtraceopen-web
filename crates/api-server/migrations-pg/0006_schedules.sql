CREATE TABLE schedules (
  name              text PRIMARY KEY,
  cron              text NOT NULL,
  selector_json     jsonb NOT NULL,
  rate_pct          integer NOT NULL CHECK (rate_pct BETWEEN 1 AND 100),
  jitter_seconds    integer NOT NULL DEFAULT 0,
  cooldown_seconds  integer NOT NULL DEFAULT 0,
  enabled           boolean NOT NULL DEFAULT true,
  last_fired_at     timestamptz NULL,
  next_fire_at      timestamptz NOT NULL,
  created_at        timestamptz NOT NULL DEFAULT now(),
  updated_at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX schedules_due_idx ON schedules (next_fire_at) WHERE enabled = true;
