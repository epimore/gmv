-- DESTRUCTIVE MANUAL OPERATION FOR MYSQL ONLY.
-- Stop Guard writes and back up the database before running this script.
-- MySQL DDL implicitly commits; this script cannot be transactionally rolled back.
-- The legacy tables have no named secondary indexes. Their primary-key indexes
-- are owned by the tables and are removed by DROP TABLE.

SELECT table_name
FROM information_schema.tables
WHERE table_schema = DATABASE()
  AND table_name LIKE 'guard\_%'
ORDER BY table_name;

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

SELECT table_name
FROM information_schema.tables
WHERE table_schema = DATABASE()
  AND table_name LIKE 'guard\_%'
ORDER BY table_name;

SELECT version, name
FROM _base_db_migrations
ORDER BY version;
