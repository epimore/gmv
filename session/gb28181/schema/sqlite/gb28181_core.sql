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

CREATE TABLE IF NOT EXISTS gb28181_enum_code (
    id VARCHAR(32) NOT NULL PRIMARY KEY,
    parent_id VARCHAR(32) NULL,
    name VARCHAR(128) NOT NULL,
    value_start VARCHAR(16) NOT NULL,
    value_end VARCHAR(16) NOT NULL,
    remark VARCHAR(255) NULL,
    seq INTEGER NULL DEFAULT 0,
    status INTEGER NOT NULL DEFAULT 1,
    created_at DATETIME NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_gb28181_enum_code_parent ON gb28181_enum_code (parent_id);
CREATE INDEX IF NOT EXISTS idx_gb28181_enum_code_value ON gb28181_enum_code (value_start, value_end, status);

CREATE TABLE IF NOT EXISTS gb28181_resource_confirmation (
    device_id VARCHAR(20) NOT NULL,
    resource_id VARCHAR(32) NOT NULL,
    resource_kind VARCHAR(32) NOT NULL,
    owner_scope VARCHAR(16) NOT NULL,
    owner_id VARCHAR(32) NOT NULL,
    status INTEGER NOT NULL DEFAULT 1,
    suggested_enum_id VARCHAR(32) NULL,
    source_parent_id VARCHAR(32) NULL,
    confirmed_by VARCHAR(64) NOT NULL,
    confirmed_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    remark VARCHAR(255) NULL,
    create_time DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    update_time DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (device_id, resource_id)
);

CREATE INDEX IF NOT EXISTS idx_gb28181_resource_confirmation_owner
    ON gb28181_resource_confirmation (device_id, owner_scope, owner_id, status);
CREATE INDEX IF NOT EXISTS idx_gb28181_resource_confirmation_kind
    ON gb28181_resource_confirmation (device_id, resource_kind, status);

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
    playback_id VARCHAR(64) NULL,
    playback_start_sec BIGINT NULL,
    playback_end_sec BIGINT NULL,
    playback_generation BIGINT NULL,
    mansrtsp_cseq BIGINT NULL,
    acknowledged_position_sec BIGINT NULL,
    desired_rate_milli BIGINT NULL,
    acknowledged_rate_milli BIGINT NULL,
    playback_state VARCHAR(16) NULL,
    pause_expire_at DATETIME NULL,
    last_control_operation_id VARCHAR(128) NULL,
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
