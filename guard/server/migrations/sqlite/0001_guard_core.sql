CREATE TABLE IF NOT EXISTS guard_node (
  node_id VARCHAR(128) NOT NULL PRIMARY KEY,
  instance_id VARCHAR(128) NOT NULL,
  node_kind VARCHAR(32) NOT NULL,
  connection_state VARCHAR(32) NOT NULL,
  health_state VARCHAR(32) NOT NULL,
  scheduling_state VARCHAR(32) NOT NULL,
  last_seen_at_ms BIGINT NOT NULL,
  generation BIGINT NOT NULL,
  sequence BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS guard_lease (
  lease_id VARCHAR(128) NOT NULL PRIMARY KEY,
  route_id VARCHAR(128) NOT NULL,
  resource_id VARCHAR(255) NOT NULL,
  node_id VARCHAR(128) NOT NULL,
  instance_id VARCHAR(128) NOT NULL,
  idempotency_key VARCHAR(128) NOT NULL,
  state VARCHAR(32) NOT NULL,
  expires_at_ms BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS guard_route (
  route_id VARCHAR(128) NOT NULL PRIMARY KEY,
  resource_id VARCHAR(255) NOT NULL,
  node_id VARCHAR(128) NOT NULL,
  instance_id VARCHAR(128) NOT NULL,
  state VARCHAR(32) NOT NULL,
  desired_generation BIGINT NOT NULL,
  observed_generation BIGINT NOT NULL,
  observed_sequence BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS guard_event (
  event_id VARCHAR(128) NOT NULL PRIMARY KEY,
  topic VARCHAR(255) NOT NULL,
  priority INTEGER NOT NULL,
  payload BLOB NOT NULL
);
