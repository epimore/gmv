CREATE TABLE IF NOT EXISTS guard_integration (
  integration_id VARCHAR(128) NOT NULL PRIMARY KEY,
  integration_kind VARCHAR(32) NOT NULL,
  name VARCHAR(255) NOT NULL,
  enabled INTEGER NOT NULL,
  endpoint TEXT NOT NULL,
  config_text TEXT NOT NULL,
  encrypted_secret TEXT,
  created_at_ms BIGINT NOT NULL,
  updated_at_ms BIGINT NOT NULL
);
