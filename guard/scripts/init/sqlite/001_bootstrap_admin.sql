-- 先使用 guard/examples/hash_password 生成 Argon2id 哈希，再替换下面两个占位值。
INSERT INTO guard_user(username, role, password_hash, enabled, created_at_ms, updated_at_ms)
SELECT '<admin-username>', 'admin', '<argon2id-hash>', 1, 0, 0
WHERE NOT EXISTS (SELECT 1 FROM guard_user);
