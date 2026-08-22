DELETE FROM guard_mqtt_runtime_state;
DELETE FROM guard_mqtt_runtime_revision;
DELETE FROM guard_integration_credential;

CREATE TABLE IF NOT EXISTS guard_integration_master_key (
  slot VARCHAR(32) NOT NULL PRIMARY KEY,
  key_material VARCHAR(64) NOT NULL,
  key_version BIGINT NOT NULL,
  created_at_ms BIGINT NOT NULL,
  updated_by VARCHAR(128) NOT NULL,
  updated_at_ms BIGINT NOT NULL
);
