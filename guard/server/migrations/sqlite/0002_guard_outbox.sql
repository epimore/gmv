CREATE TABLE IF NOT EXISTS guard_outbox (
  outbox_id VARCHAR(128) NOT NULL PRIMARY KEY,
  event_id VARCHAR(128) NOT NULL,
  destination_kind VARCHAR(32) NOT NULL,
  destination TEXT NOT NULL,
  payload BLOB NOT NULL,
  state VARCHAR(32) NOT NULL,
  attempts INTEGER NOT NULL,
  next_attempt_at_ms BIGINT NOT NULL,
  last_error TEXT,
  created_at_ms BIGINT NOT NULL,
  updated_at_ms BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS guard_command (
  command_id VARCHAR(128) NOT NULL PRIMARY KEY,
  expires_at_ms BIGINT NOT NULL,
  created_at_ms BIGINT NOT NULL
);
