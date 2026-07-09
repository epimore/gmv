-- Rebuild legacy uppercase GB28181 SQLite schema into the lowercase schema.
-- Use only for existing preview/test SQLite databases created before the lowercase naming refactor.
PRAGMA foreign_keys = OFF;
BEGIN TRANSACTION;

DROP TABLE IF EXISTS tmp_legacy_gb28181_seq_code;
ALTER TABLE GB28181_SEQ_CODE RENAME TO tmp_legacy_gb28181_seq_code;
DROP TABLE IF EXISTS tmp_legacy_gb28181_oauth;
ALTER TABLE GB28181_OAUTH RENAME TO tmp_legacy_gb28181_oauth;
DROP TABLE IF EXISTS tmp_legacy_gb28181_device;
ALTER TABLE GB28181_DEVICE RENAME TO tmp_legacy_gb28181_device;
DROP TABLE IF EXISTS tmp_legacy_gb28181_device_channel;
ALTER TABLE GB28181_DEVICE_CHANNEL RENAME TO tmp_legacy_gb28181_device_channel;
DROP TABLE IF EXISTS tmp_legacy_gb28181_device_channel_conf;
ALTER TABLE GB28181_DEVICE_CHANNEL_CONF RENAME TO tmp_legacy_gb28181_device_channel_conf;
DROP TABLE IF EXISTS tmp_legacy_gb28181_device_ptz_preset;
ALTER TABLE GB28181_DEVICE_PTZ_PRESET RENAME TO tmp_legacy_gb28181_device_ptz_preset;
DROP TABLE IF EXISTS tmp_legacy_gb28181_file_info;
ALTER TABLE GB28181_FILE_INFO RENAME TO tmp_legacy_gb28181_file_info;
DROP TABLE IF EXISTS tmp_legacy_gb28181_record;
ALTER TABLE GB28181_RECORD RENAME TO tmp_legacy_gb28181_record;
DROP TABLE IF EXISTS tmp_legacy_gb28181_sip_dialog_session;
ALTER TABLE GB28181_SIP_DIALOG_SESSION RENAME TO tmp_legacy_gb28181_sip_dialog_session;

-- seq_name uses domain_id:LIVE or domain_id:BACK; prefix_code keeps the numeric ssrc prefix.
CREATE TABLE IF NOT EXISTS gb28181_seq_code (
    seq_id INTEGER PRIMARY KEY AUTOINCREMENT,
    seq_name VARCHAR(64) NOT NULL UNIQUE,
    init_value BIGINT NOT NULL,
    current_value BIGINT NOT NULL,
    increment_value INTEGER NOT NULL DEFAULT 1,
    prefix_code VARCHAR(64) NULL,
    code_lenth INTEGER NULL,
    remark VARCHAR(256) NULL,
    create_date DATETIME NULL
);

CREATE TABLE IF NOT EXISTS gb28181_oauth (
    device_id VARCHAR(20) NOT NULL PRIMARY KEY,
    domain_id VARCHAR(20) NOT NULL DEFAULT '34020000002000000001',
    domain VARCHAR(20) NOT NULL,
    longitude DECIMAL(12, 8) NULL,
    latitude DECIMAL(12, 8) NULL,
    address VARCHAR(255) NULL,
    pwd VARCHAR(120) NULL,
    pwd_check INTEGER NULL,
    alias VARCHAR(32) NULL,
    status INTEGER NULL,
    heartbeat_sec INTEGER NULL,
    del INTEGER NULL,
    create_time DATETIME NULL,
    tenant_id INTEGER NULL,
    sys_org_code VARCHAR(64) NULL,
    create_by VARCHAR(64) NULL,
    update_by VARCHAR(64) NULL,
    update_time DATETIME NULL
);

CREATE TABLE IF NOT EXISTS gb28181_device (
    device_id VARCHAR(20) NOT NULL PRIMARY KEY,
    transport VARCHAR(3) NULL,
    register_expires INTEGER NULL,
    register_time DATETIME NULL,
    local_addr VARCHAR(32) NULL,
    contact_uri VARCHAR(128) NULL,
    enable_lr INTEGER NULL,
    device_type VARCHAR(16) NULL,
    manufacturer VARCHAR(32) NULL,
    model VARCHAR(64) NULL,
    firmware VARCHAR(64) NULL,
    max_camera INTEGER NULL,
    online_expire_time DATETIME NULL,
    gb_version VARCHAR(32) NULL DEFAULT '2.0',
    last_update_time DATETIME NULL,
    create_time DATETIME NULL,
    tenant_id INTEGER NULL,
    sys_org_code VARCHAR(64) NULL,
    create_by VARCHAR(64) NULL,
    update_by VARCHAR(64) NULL,
    update_time DATETIME NULL
);

CREATE TABLE IF NOT EXISTS gb28181_device_channel (
    device_id VARCHAR(20) NOT NULL,
    channel_id VARCHAR(20) NOT NULL,
    name VARCHAR(32) NULL,
    manufacturer VARCHAR(32) NULL,
    model VARCHAR(64) NULL,
    owner VARCHAR(32) NULL,
    status VARCHAR(32) NULL DEFAULT 'ON',
    civil_code VARCHAR(32) NULL,
    address VARCHAR(32) NULL,
    parental CHAR(1) NULL,
    block VARCHAR(32) NULL,
    parent_id VARCHAR(32) NULL,
    ip_address VARCHAR(32) NULL,
    port INTEGER NULL,
    password VARCHAR(32) NULL,
    longitude DECIMAL(12, 6) NULL,
    latitude DECIMAL(12, 6) NULL,
    ptz_type CHAR(1) NULL,
    supply_light_type CHAR(1) NULL,
    PRIMARY KEY (device_id, channel_id)
);

CREATE TABLE IF NOT EXISTS gb28181_device_channel_conf (
    device_id VARCHAR(20) NOT NULL,
    channel_id VARCHAR(20) NOT NULL,
    alias_name VARCHAR(16) NULL,
    ptz_enable INTEGER NULL DEFAULT 2,
    talk_enable INTEGER NULL DEFAULT 2,
    audio_enable INTEGER NULL DEFAULT 2,
    snapshot_enable INTEGER NULL DEFAULT 2,
    record_enable INTEGER NULL DEFAULT 2,
    playback_enable INTEGER NULL DEFAULT 2,
    alarm_enable INTEGER NULL DEFAULT 2,
    biz_enable INTEGER NULL DEFAULT 1,
    sort_no INTEGER NULL DEFAULT 0,
    over_pic_id BIGINT NULL,
    create_time DATETIME NULL DEFAULT CURRENT_TIMESTAMP,
    update_time DATETIME NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (device_id, channel_id),
    FOREIGN KEY (device_id, channel_id) REFERENCES gb28181_device_channel (device_id, channel_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_gb28181_dcc_sort ON gb28181_device_channel_conf (device_id, sort_no, channel_id);

CREATE TABLE IF NOT EXISTS gb28181_device_ptz_preset (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id VARCHAR(20) NOT NULL,
    channel_id VARCHAR(20) NOT NULL,
    preset_no INTEGER NOT NULL,
    preset_name VARCHAR(64) NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    sort_no INTEGER NULL DEFAULT 0,
    remark VARCHAR(255) NULL,
    create_time DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    update_time DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (device_id, channel_id, preset_no),
    FOREIGN KEY (device_id, channel_id) REFERENCES gb28181_device_channel (device_id, channel_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_gb28181_ptz_preset_channel ON gb28181_device_ptz_preset (device_id, channel_id, enabled, sort_no);

CREATE TABLE IF NOT EXISTS gb28181_file_info (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id VARCHAR(20) NOT NULL,
    channel_id VARCHAR(20) NOT NULL,
    biz_time DATETIME NULL,
    biz_id VARCHAR(128) NOT NULL,
    file_type INTEGER NULL,
    file_size BIGINT NULL,
    file_name VARCHAR(128) NOT NULL,
    file_format VARCHAR(32) NULL,
    dir_path VARCHAR(255) NOT NULL,
    abs_path VARCHAR(255) NULL,
    note VARCHAR(128) NULL,
    is_del INTEGER NULL DEFAULT 0,
    create_time DATETIME NULL
);

CREATE INDEX IF NOT EXISTS idx_gb28181_file_dc ON gb28181_file_info (device_id, channel_id);
CREATE INDEX IF NOT EXISTS idx_gb28181_file_device_channel_id ON gb28181_file_info (device_id, channel_id, id DESC);

CREATE TABLE IF NOT EXISTS gb28181_record (
    biz_id VARCHAR(128) NOT NULL PRIMARY KEY,
    device_id VARCHAR(20) NOT NULL,
    channel_id VARCHAR(20) NOT NULL,
    user_id VARCHAR(32) NULL,
    st DATETIME NULL,
    et DATETIME NULL,
    speed INTEGER NULL,
    ct DATETIME NULL,
    state INTEGER NULL,
    lt DATETIME NULL,
    stream_app_name VARCHAR(64) NULL
);

CREATE TABLE IF NOT EXISTS gb28181_sip_dialog_session (
    stream_id VARCHAR(64) NOT NULL PRIMARY KEY,
    device_id VARCHAR(32) NOT NULL,
    channel_id VARCHAR(32) NOT NULL,
    session_type VARCHAR(16) NOT NULL,
    signal_node_id VARCHAR(64) NOT NULL,
    media_node_id VARCHAR(64) NOT NULL,
    ssrc VARCHAR(16) NULL,
    call_id VARCHAR(128) NOT NULL,
    local_uri VARCHAR(256) NOT NULL,
    remote_uri VARCHAR(256) NOT NULL,
    local_tag VARCHAR(128) NOT NULL,
    remote_tag VARCHAR(128) NULL,
    local_cseq BIGINT NOT NULL DEFAULT 1,
    remote_cseq BIGINT NULL,
    contact_uri VARCHAR(256) NULL,
    route_set TEXT NULL,
    local_sip_addr VARCHAR(64) NOT NULL,
    remote_sip_addr VARCHAR(64) NOT NULL,
    transport VARCHAR(8) NOT NULL,
    state VARCHAR(32) NOT NULL,
    established_at DATETIME NULL,
    last_seen_at DATETIME NOT NULL,
    expire_at DATETIME NOT NULL,
    version BIGINT NOT NULL DEFAULT 0,
    created_at DATETIME NOT NULL,
    updated_at DATETIME NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_gb28181_sip_dialog_device_state ON gb28181_sip_dialog_session (device_id, state);
CREATE INDEX IF NOT EXISTS idx_gb28181_sip_dialog_call_id ON gb28181_sip_dialog_session (call_id);
CREATE INDEX IF NOT EXISTS idx_gb28181_sip_dialog_owner_state_expire ON gb28181_sip_dialog_session (signal_node_id, state, expire_at);
CREATE INDEX IF NOT EXISTS idx_gb28181_sip_dialog_owner_ssrc_state_expire ON gb28181_sip_dialog_session (signal_node_id, ssrc, state, expire_at);
CREATE INDEX IF NOT EXISTS idx_gb28181_sip_dialog_owner ON gb28181_sip_dialog_session (signal_node_id, state, stream_id);
CREATE INDEX IF NOT EXISTS idx_gb28181_sip_dialog_ssrc ON gb28181_sip_dialog_session (signal_node_id, media_node_id, ssrc, state, expire_at);

INSERT INTO gb28181_seq_code (seq_id, seq_name, init_value, current_value, increment_value, prefix_code, code_lenth, remark, create_date)
SELECT SEQ_ID, SEQ_NAME, INIT_VALUE, CURRENT_VALUE, INCREMENT_VALUE, PREFIX_CODE, CODE_LENTH, REMARK, CREATE_DATE FROM tmp_legacy_gb28181_seq_code;

INSERT INTO gb28181_oauth (device_id, domain_id, domain, longitude, latitude, address, pwd, pwd_check, alias, status, heartbeat_sec, del, create_time, tenant_id, sys_org_code, create_by, update_by, update_time)
SELECT DEVICE_ID, DOMAIN_ID, DOMAIN, LONGITUDE, LATITUDE, ADDRESS, PWD, PWD_CHECK, ALIAS, STATUS, HEARTBEAT_SEC, DEL, CREATE_TIME, TENANT_ID, SYS_ORG_CODE, CREATE_BY, UPDATE_BY, UPDATE_TIME FROM tmp_legacy_gb28181_oauth;

INSERT INTO gb28181_device (device_id, transport, register_expires, register_time, local_addr, contact_uri, enable_lr, device_type, manufacturer, model, firmware, max_camera, online_expire_time, gb_version, last_update_time, create_time, tenant_id, sys_org_code, create_by, update_by, update_time)
SELECT DEVICE_ID, TRANSPORT, REGISTER_EXPIRES, REGISTER_TIME, LOCAL_ADDR, CONTACT_URI, ENABLE_LR, DEVICE_TYPE, MANUFACTURER, MODEL, FIRMWARE, MAX_CAMERA, ONLINE_EXPIRE_TIME, GB_VERSION, LAST_UPDATE_TIME, CREATE_TIME, TENANT_ID, SYS_ORG_CODE, CREATE_BY, UPDATE_BY, UPDATE_TIME FROM tmp_legacy_gb28181_device;

INSERT INTO gb28181_device_channel (device_id, channel_id, name, manufacturer, model, owner, status, civil_code, address, parental, block, parent_id, ip_address, port, password, longitude, latitude, ptz_type, supply_light_type)
SELECT DEVICE_ID, CHANNEL_ID, NAME, MANUFACTURER, MODEL, OWNER, STATUS, CIVIL_CODE, ADDRESS, PARENTAL, BLOCK, PARENT_ID, IP_ADDRESS, PORT, PASSWORD, LONGITUDE, LATITUDE, PTZ_TYPE, SUPPLY_LIGHT_TYPE FROM tmp_legacy_gb28181_device_channel;

INSERT INTO gb28181_device_channel_conf (device_id, channel_id, alias_name, ptz_enable, talk_enable, audio_enable, snapshot_enable, record_enable, playback_enable, alarm_enable, biz_enable, sort_no, over_pic_id, create_time, update_time)
SELECT DEVICE_ID, CHANNEL_ID, ALIAS_NAME, PTZ_ENABLE, TALK_ENABLE, AUDIO_ENABLE, SNAPSHOT_ENABLE, RECORD_ENABLE, PLAYBACK_ENABLE, ALARM_ENABLE, BIZ_ENABLE, SORT_NO, OVER_PIC_ID, CREATE_TIME, UPDATE_TIME FROM tmp_legacy_gb28181_device_channel_conf;

INSERT INTO gb28181_device_ptz_preset (id, device_id, channel_id, preset_no, preset_name, enabled, sort_no, remark, create_time, update_time)
SELECT ID, DEVICE_ID, CHANNEL_ID, PRESET_NO, PRESET_NAME, ENABLED, SORT_NO, REMARK, CREATE_TIME, UPDATE_TIME FROM tmp_legacy_gb28181_device_ptz_preset;

INSERT INTO gb28181_file_info (id, device_id, channel_id, biz_time, biz_id, file_type, file_size, file_name, file_format, dir_path, abs_path, note, is_del, create_time)
SELECT ID, DEVICE_ID, CHANNEL_ID, BIZ_TIME, BIZ_ID, FILE_TYPE, FILE_SIZE, FILE_NAME, FILE_FORMAT, DIR_PATH, ABS_PATH, NOTE, IS_DEL, CREATE_TIME FROM tmp_legacy_gb28181_file_info;

INSERT INTO gb28181_record (biz_id, device_id, channel_id, user_id, st, et, speed, ct, state, lt, stream_app_name)
SELECT BIZ_ID, DEVICE_ID, CHANNEL_ID, USER_ID, ST, ET, SPEED, CT, STATE, LT, STREAM_APP_NAME FROM tmp_legacy_gb28181_record;

INSERT INTO gb28181_sip_dialog_session (stream_id, device_id, channel_id, session_type, signal_node_id, media_node_id, ssrc, call_id, local_uri, remote_uri, local_tag, remote_tag, local_cseq, remote_cseq, contact_uri, route_set, local_sip_addr, remote_sip_addr, transport, state, established_at, last_seen_at, expire_at, version, created_at, updated_at)
SELECT STREAM_ID, DEVICE_ID, CHANNEL_ID, SESSION_TYPE, SIGNAL_NODE_ID, MEDIA_NODE_ID, SSRC, CALL_ID, LOCAL_URI, REMOTE_URI, LOCAL_TAG, REMOTE_TAG, LOCAL_CSEQ, REMOTE_CSEQ, CONTACT_URI, ROUTE_SET, LOCAL_SIP_ADDR, REMOTE_SIP_ADDR, TRANSPORT, STATE, ESTABLISHED_AT, LAST_SEEN_AT, EXPIRE_AT, VERSION, CREATED_AT, UPDATED_AT FROM tmp_legacy_gb28181_sip_dialog_session;

DROP TABLE tmp_legacy_gb28181_sip_dialog_session;
DROP TABLE tmp_legacy_gb28181_record;
DROP TABLE tmp_legacy_gb28181_file_info;
DROP TABLE tmp_legacy_gb28181_device_ptz_preset;
DROP TABLE tmp_legacy_gb28181_device_channel_conf;
DROP TABLE tmp_legacy_gb28181_device_channel;
DROP TABLE tmp_legacy_gb28181_device;
DROP TABLE tmp_legacy_gb28181_oauth;
DROP TABLE tmp_legacy_gb28181_seq_code;

COMMIT;
PRAGMA foreign_keys = ON;
