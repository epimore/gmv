ALTER TABLE guard_command ADD COLUMN request_hash VARCHAR(64) NOT NULL DEFAULT '';
ALTER TABLE guard_command ADD COLUMN http_status BIGINT NULL;
ALTER TABLE guard_command ADD COLUMN response_body BLOB NULL;

CREATE INDEX IF NOT EXISTS idx_guard_command_integration_created
  ON guard_command(integration_id, created_at_ms);
