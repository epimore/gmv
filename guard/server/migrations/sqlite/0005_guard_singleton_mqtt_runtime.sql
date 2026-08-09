CREATE TABLE IF NOT EXISTS guard_integration_slot (
  slot VARCHAR(32) NOT NULL PRIMARY KEY,
  integration_id VARCHAR(128) NULL UNIQUE,
  updated_by VARCHAR(128) NOT NULL,
  updated_at_ms BIGINT NOT NULL,
  FOREIGN KEY (integration_id) REFERENCES guard_integration(integration_id)
);

INSERT OR IGNORE INTO guard_integration_slot(slot, integration_id, updated_by, updated_at_ms)
VALUES ('business', NULL, 'migration', 0);

CREATE TABLE IF NOT EXISTS guard_mqtt_runtime_revision (
  slot VARCHAR(32) NOT NULL,
  revision BIGINT NOT NULL,
  protocol_version VARCHAR(8) NOT NULL,
  broker VARCHAR(255) NOT NULL,
  port BIGINT NOT NULL,
  client_id VARCHAR(255) NOT NULL,
  username VARCHAR(255) NULL,
  password_ciphertext TEXT NULL,
  tls INTEGER NOT NULL,
  publish_event_ttl_sec BIGINT NOT NULL,
  created_by VARCHAR(128) NOT NULL,
  created_at_ms BIGINT NOT NULL,
  PRIMARY KEY (slot, revision),
  FOREIGN KEY (slot) REFERENCES guard_integration_slot(slot)
);

CREATE TABLE IF NOT EXISTS guard_mqtt_runtime_state (
  slot VARCHAR(32) NOT NULL PRIMARY KEY,
  desired_revision BIGINT NOT NULL,
  active_revision BIGINT NULL,
  config_version BIGINT NOT NULL,
  apply_state VARCHAR(32) NOT NULL,
  last_error_code VARCHAR(64) NULL,
  last_error_summary VARCHAR(512) NULL,
  last_transition_at_ms BIGINT NOT NULL,
  updated_by VARCHAR(128) NOT NULL,
  updated_at_ms BIGINT NOT NULL,
  FOREIGN KEY (slot) REFERENCES guard_integration_slot(slot)
);
