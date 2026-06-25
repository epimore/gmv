CREATE TABLE IF NOT EXISTS guard_user (
  username VARCHAR(128) NOT NULL PRIMARY KEY,
  role VARCHAR(32) NOT NULL,
  password_hash TEXT NOT NULL,
  enabled INTEGER NOT NULL,
  created_at_ms BIGINT NOT NULL,
  updated_at_ms BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS guard_service_credential (
  node_id VARCHAR(128) NOT NULL PRIMARY KEY,
  node_kind VARCHAR(32) NOT NULL,
  token_hash TEXT NOT NULL,
  enabled INTEGER NOT NULL,
  created_at_ms BIGINT NOT NULL,
  updated_at_ms BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS guard_ui_session (
  session_hash VARCHAR(128) NOT NULL PRIMARY KEY,
  username VARCHAR(128) NOT NULL,
  csrf_hash VARCHAR(128) NOT NULL,
  expires_at_ms BIGINT NOT NULL,
  created_at_ms BIGINT NOT NULL
);
