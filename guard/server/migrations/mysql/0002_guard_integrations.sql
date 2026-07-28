CREATE TABLE IF NOT EXISTS guard_integration (
  integration_id VARCHAR(128) NOT NULL PRIMARY KEY,
  name VARCHAR(255) NOT NULL,
  transport VARCHAR(16) NOT NULL,
  inbound_enabled INTEGER NOT NULL,
  outbound_enabled INTEGER NOT NULL,
  enabled INTEGER NOT NULL,
  scopes TEXT NOT NULL,
  expires_at_ms BIGINT NULL,
  config_version BIGINT NOT NULL,
  created_by VARCHAR(128) NOT NULL,
  created_at_ms BIGINT NOT NULL,
  updated_at_ms BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS guard_integration_credential (
  credential_id VARCHAR(128) NOT NULL PRIMARY KEY,
  access_key VARCHAR(128) NOT NULL UNIQUE,
  integration_id VARCHAR(128) NOT NULL,
  purpose VARCHAR(32) NOT NULL,
  secret_ciphertext TEXT NOT NULL,
  key_version BIGINT NOT NULL,
  status VARCHAR(16) NOT NULL,
  not_before_ms BIGINT NOT NULL,
  expires_at_ms BIGINT NULL,
  revoked_at_ms BIGINT NULL,
  created_by VARCHAR(128) NOT NULL,
  created_at_ms BIGINT NOT NULL,
  updated_at_ms BIGINT NOT NULL,
  FOREIGN KEY (integration_id) REFERENCES guard_integration(integration_id)
);

CREATE INDEX idx_guard_integration_credential_app
  ON guard_integration_credential(integration_id, purpose, status);

CREATE TABLE IF NOT EXISTS guard_integration_http (
  integration_id VARCHAR(128) NOT NULL PRIMARY KEY,
  callback_url TEXT NULL,
  callback_timeout_ms BIGINT NOT NULL,
  private_network_policy TEXT NOT NULL,
  private_network_allowlist TEXT NOT NULL,
  max_attempts BIGINT NOT NULL,
  event_ttl_ms BIGINT NOT NULL,
  max_response_bytes BIGINT NOT NULL,
  updated_at_ms BIGINT NOT NULL,
  FOREIGN KEY (integration_id) REFERENCES guard_integration(integration_id)
);

CREATE TABLE IF NOT EXISTS guard_integration_mqtt (
  integration_id VARCHAR(128) NOT NULL PRIMARY KEY,
  protocol_version VARCHAR(8) NOT NULL,
  allowed_actions TEXT NOT NULL,
  command_topic VARCHAR(512) NOT NULL UNIQUE,
  result_topic VARCHAR(512) NOT NULL UNIQUE,
  event_topic_prefix VARCHAR(512) NOT NULL,
  updated_at_ms BIGINT NOT NULL,
  FOREIGN KEY (integration_id) REFERENCES guard_integration(integration_id)
);

CREATE TABLE IF NOT EXISTS guard_integration_mapping (
  mapping_id VARCHAR(128) NOT NULL PRIMARY KEY,
  integration_id VARCHAR(128) NOT NULL,
  direction VARCHAR(16) NOT NULL,
  source_type VARCHAR(255) NOT NULL,
  schema_version VARCHAR(32) NOT NULL,
  destination_kind VARCHAR(16) NOT NULL,
  destination VARCHAR(1024) NOT NULL,
  payload_profile VARCHAR(128) NOT NULL,
  enabled INTEGER NOT NULL,
  created_at_ms BIGINT NOT NULL,
  updated_at_ms BIGINT NOT NULL,
  FOREIGN KEY (integration_id) REFERENCES guard_integration(integration_id),
  UNIQUE (integration_id, direction, source_type, destination_kind, destination)
);

CREATE INDEX idx_guard_integration_mapping_source
  ON guard_integration_mapping(source_type, enabled);

CREATE TABLE IF NOT EXISTS guard_integration_audit (
  audit_id VARCHAR(128) NOT NULL PRIMARY KEY,
  integration_id VARCHAR(128) NULL,
  actor VARCHAR(128) NOT NULL,
  action VARCHAR(64) NOT NULL,
  target_id VARCHAR(128) NOT NULL,
  outcome VARCHAR(32) NOT NULL,
  detail_summary TEXT NOT NULL,
  created_at_ms BIGINT NOT NULL
);

CREATE INDEX idx_guard_integration_audit_created
  ON guard_integration_audit(created_at_ms, audit_id);

CREATE TABLE IF NOT EXISTS guard_integration_delivery (
  event_id VARCHAR(128) NOT NULL,
  mapping_id VARCHAR(128) NOT NULL,
  expires_at_ms BIGINT NOT NULL,
  created_at_ms BIGINT NOT NULL,
  PRIMARY KEY (event_id, mapping_id)
);

CREATE INDEX idx_guard_integration_delivery_expires
  ON guard_integration_delivery(expires_at_ms);

ALTER TABLE guard_command ADD COLUMN integration_id VARCHAR(128) NOT NULL DEFAULT '';
ALTER TABLE guard_command ADD COLUMN operation_id VARCHAR(128) NOT NULL DEFAULT '';
ALTER TABLE guard_command ADD COLUMN action VARCHAR(128) NOT NULL DEFAULT '';
ALTER TABLE guard_command ADD COLUMN state VARCHAR(32) NOT NULL DEFAULT 'CLAIMED';
ALTER TABLE guard_command ADD COLUMN updated_at_ms BIGINT NOT NULL DEFAULT 0;

ALTER TABLE guard_outbox ADD COLUMN integration_id VARCHAR(128) NOT NULL DEFAULT '';
ALTER TABLE guard_outbox ADD COLUMN mapping_id VARCHAR(128) NOT NULL DEFAULT '';
ALTER TABLE guard_outbox ADD COLUMN expires_at_ms BIGINT NULL;

CREATE INDEX idx_guard_outbox_integration_state
  ON guard_outbox(integration_id, state, next_attempt_at_ms);
