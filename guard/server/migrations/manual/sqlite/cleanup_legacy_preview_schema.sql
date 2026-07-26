-- DESTRUCTIVE MANUAL OPERATION FOR SQLITE ONLY.
-- Stop Guard and back up the database file before running this script.
-- The legacy tables have no named secondary indexes. Their primary-key
-- sqlite_autoindex_* entries are owned by the tables and disappear with them.

SELECT name
FROM sqlite_master
WHERE type = 'table' AND name LIKE 'guard\_%' ESCAPE '\'
ORDER BY name;

BEGIN IMMEDIATE;

DROP TABLE IF EXISTS guard_ui_session;
DROP TABLE IF EXISTS guard_service_credential;
DROP TABLE IF EXISTS guard_system_setting;
DROP TABLE IF EXISTS guard_integration;
DROP TABLE IF EXISTS guard_event;
DROP TABLE IF EXISTS guard_route;
DROP TABLE IF EXISTS guard_lease;
DROP TABLE IF EXISTS guard_node;

DELETE FROM _base_db_migrations
WHERE (version = 1 AND name = 'guard_core')
   OR (version = 2 AND name = 'guard_outbox')
   OR (version = 3 AND name = 'guard_security')
   OR (version = 4 AND name = 'guard_integrations')
   OR (version = 5 AND name = 'guard_settings')
   OR (version = 6 AND name = 'guard_user_profile');

COMMIT;

SELECT name
FROM sqlite_master
WHERE type = 'table' AND name LIKE 'guard\_%' ESCAPE '\'
ORDER BY name;

SELECT version, name
FROM _base_db_migrations
ORDER BY version;
