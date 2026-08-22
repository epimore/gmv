ALTER TABLE guard_integration ADD COLUMN slot VARCHAR(32) NULL;

UPDATE guard_integration
SET slot = (
  SELECT guard_integration_slot.slot
  FROM guard_integration_slot
  WHERE guard_integration_slot.integration_id = guard_integration.integration_id
)
WHERE EXISTS (
  SELECT 1
  FROM guard_integration_slot
  WHERE guard_integration_slot.integration_id = guard_integration.integration_id
);

CREATE UNIQUE INDEX idx_guard_integration_slot
  ON guard_integration(slot);

CREATE TABLE guard_mqtt_runtime_revision_v9 (
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
  FOREIGN KEY (slot) REFERENCES guard_integration(slot)
);

INSERT INTO guard_mqtt_runtime_revision_v9(
  slot, revision, protocol_version, broker, port, client_id, username,
  password_ciphertext, tls, publish_event_ttl_sec, created_by, created_at_ms
)
SELECT
  slot, revision, protocol_version, broker, port, client_id, username,
  password_ciphertext, tls, publish_event_ttl_sec, created_by, created_at_ms
FROM guard_mqtt_runtime_revision;

CREATE TABLE guard_mqtt_runtime_state_v9 (
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
  FOREIGN KEY (slot) REFERENCES guard_integration(slot)
);

INSERT INTO guard_mqtt_runtime_state_v9(
  slot, desired_revision, active_revision, config_version, apply_state,
  last_error_code, last_error_summary, last_transition_at_ms, updated_by, updated_at_ms
)
SELECT
  slot, desired_revision, active_revision, config_version, apply_state,
  last_error_code, last_error_summary, last_transition_at_ms, updated_by, updated_at_ms
FROM guard_mqtt_runtime_state;

DROP TABLE guard_mqtt_runtime_state;
DROP TABLE guard_mqtt_runtime_revision;

ALTER TABLE guard_mqtt_runtime_revision_v9 RENAME TO guard_mqtt_runtime_revision;
ALTER TABLE guard_mqtt_runtime_state_v9 RENAME TO guard_mqtt_runtime_state;

DROP TABLE guard_integration_mqtt;
DROP TABLE guard_integration_slot;
