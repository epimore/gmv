ALTER TABLE guard_integration
  ADD COLUMN slot VARCHAR(32) NULL;

UPDATE guard_integration AS integration
JOIN guard_integration_slot AS integration_slot
  ON integration_slot.integration_id = integration.integration_id
SET integration.slot = integration_slot.slot;

CREATE UNIQUE INDEX idx_guard_integration_slot
  ON guard_integration(slot);

ALTER TABLE guard_mqtt_runtime_revision
  DROP FOREIGN KEY guard_mqtt_runtime_revision_ibfk_1;
ALTER TABLE guard_mqtt_runtime_state
  DROP FOREIGN KEY guard_mqtt_runtime_state_ibfk_1;

ALTER TABLE guard_mqtt_runtime_revision
  ADD CONSTRAINT fk_guard_mqtt_revision_integration_slot
  FOREIGN KEY (slot) REFERENCES guard_integration(slot);
ALTER TABLE guard_mqtt_runtime_state
  ADD CONSTRAINT fk_guard_mqtt_state_integration_slot
  FOREIGN KEY (slot) REFERENCES guard_integration(slot);

DROP TABLE guard_integration_mqtt;
DROP TABLE guard_integration_slot;
