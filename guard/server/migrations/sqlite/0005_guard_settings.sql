CREATE TABLE IF NOT EXISTS guard_system_setting (
  setting_key VARCHAR(128) NOT NULL PRIMARY KEY,
  setting_value TEXT NOT NULL,
  updated_at_ms BIGINT NOT NULL
);
