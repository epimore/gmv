-- ----------------------------
-- Table structure for gb28181_seq_code
-- ----------------------------
CREATE TABLE IF NOT EXISTS `gb28181_seq_code`  (
  `seq_id` bigint(16) UNSIGNED ZEROFILL NOT NULL AUTO_INCREMENT COMMENT '序列标识',
  `seq_name` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '序列的名字，唯一，格式domain_id:LIVE或domain_id:BACK',
  `init_value` bigint UNSIGNED NOT NULL COMMENT '初始值',
  `current_value` bigint UNSIGNED NOT NULL COMMENT '当前的值',
  `increment_value` int NOT NULL DEFAULT 1 COMMENT '步长，默认为1',
  `prefix_code` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT 'ssrc数字前缀',
  `code_lenth` int NULL DEFAULT NULL COMMENT '编码长度(不含前置)，中间以0填充',
  `remark` varchar(256) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '备注',
  `create_date` datetime NULL DEFAULT NULL ON UPDATE CURRENT_TIMESTAMP COMMENT '创建时间',
  PRIMARY KEY (`seq_id`) USING BTREE,
  UNIQUE INDEX `udx_seq_name`(`seq_name` ASC) USING BTREE
) ENGINE = InnoDB AUTO_INCREMENT = 40 CHARACTER SET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci COMMENT = '公共的序列表' ROW_FORMAT = DYNAMIC;

-- ----------------------------
-- Table structure for gb28181_device
-- ----------------------------
CREATE TABLE IF NOT EXISTS `gb28181_device`  (
  `device_id` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '设备主键id',
  `transport` varchar(3) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '网络协议：TCP/UDP',
  `register_expires` int UNSIGNED NULL DEFAULT NULL COMMENT '注册有效期',
  `register_time` datetime NULL DEFAULT NULL COMMENT '最近注册时间',
  `local_addr` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备本地地址',
  `contact_uri` varchar(128) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '请求地址',
  `enable_lr` tinyint UNSIGNED NULL DEFAULT NULL COMMENT '1-是，0-否',
  `device_type` varchar(16) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备类型IPC/NVR/DVR...',
  `manufacturer` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '厂家名称',
  `model` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备型号',
  `firmware` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '固件版本',
  `max_camera` smallint UNSIGNED NULL DEFAULT NULL COMMENT '最大相机数',
  `online_expire_time` datetime NULL DEFAULT NULL COMMENT '在线过期时间',
  `gb_version` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT '2.0' COMMENT '国标版本',
  `last_update_time` datetime NULL DEFAULT NULL ON UPDATE CURRENT_TIMESTAMP COMMENT '最后更新时间',
  `create_time` datetime NULL DEFAULT NULL,
  `tenant_id` int NULL DEFAULT NULL COMMENT '租户id',
  `sys_org_code` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '机构编码',
  `create_by` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL,
  `update_by` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL,
  `update_time` datetime NULL DEFAULT NULL,
  PRIMARY KEY (`device_id`) USING BTREE
) ENGINE = InnoDB CHARACTER SET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci COMMENT = '设备主表' ROW_FORMAT = Dynamic;

-- ----------------------------
-- Table structure for gb28181_device_channel
-- ----------------------------
CREATE TABLE IF NOT EXISTS `gb28181_device_channel`  (
  `device_id` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '设备id',
  `channel_id` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '通道id',
  `name` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备名称',
  `manufacturer` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备厂商',
  `model` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备型号',
  `owner` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备归属',
  `status` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT 'ON' COMMENT '设备状态ON默认/OFF/STATUS1/ONLINE/OFFLINE....',
  `civil_code` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '行政区域',
  `address` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '安装地址',
  `parental` char(1) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '是否有子设备 1 有， 0 没有',
  `block` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '警区',
  `parent_id` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '父设备/区域/系统 id',
  `ip_address` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备/区域/系统 IP 地址',
  `port` int NULL DEFAULT NULL COMMENT '设备/区域/系统端口',
  `password` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备口令',
  `longitude` decimal(12, 6) NULL DEFAULT NULL COMMENT '经度',
  `latitude` decimal(12, 6) NULL DEFAULT NULL COMMENT '纬度',
  `ptz_type` char(1) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '摄像机类型扩展，标识摄像机类型： 1-球机； 2-半球； 3-固定枪机；4-遥控枪机,5遥控半球，6多目设备拼接通道，7多目设备分割通道。',
  `supply_light_type` char(1) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '摄像机补光属性。 1-无补光、 2-红外补光、 3-白光补光。',
  PRIMARY KEY (`device_id`, `channel_id`) USING BTREE
) ENGINE = InnoDB CHARACTER SET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci COMMENT = '摄像机通道信息' ROW_FORMAT = Dynamic;

-- ----------------------------
-- Table structure for gb28181_device_channel_conf
-- ----------------------------
CREATE TABLE IF NOT EXISTS `gb28181_device_channel_conf`  (
  `device_id` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '设备id',
  `channel_id` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '通道id',
  `alias_name` varchar(16) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '业务别名',
  `ptz_enable` tinyint UNSIGNED NULL DEFAULT 2 COMMENT '云台控制：0-禁用，1-启用，2-设备不支持',
  `talk_enable` tinyint UNSIGNED NULL DEFAULT 2 COMMENT '语音对讲：0-禁用，1-启用，2-设备不支持',
  `audio_enable` tinyint UNSIGNED NULL DEFAULT 2 COMMENT '音频播放/收音：0-禁用，1-启用，2-设备不支持',
  `snapshot_enable` tinyint UNSIGNED NULL DEFAULT 2 COMMENT '抓拍：0-禁用，1-启用，2-设备不支持',
  `record_enable` tinyint UNSIGNED NULL DEFAULT 2 COMMENT '录像：0-禁用，1-启用，2-设备不支持',
  `playback_enable` tinyint UNSIGNED NULL DEFAULT 2 COMMENT '录像回放：0-禁用，1-启用，2-设备不支持',
  `alarm_enable` tinyint UNSIGNED NULL DEFAULT 2 COMMENT '告警接收：0-禁用，1-启用，2-设备不支持',
  `biz_enable` tinyint UNSIGNED NULL DEFAULT 1 COMMENT '业务启用：0-禁用，1-启用',
  `sort_no` int NULL DEFAULT 0 COMMENT '排序号',
  `over_pic_id` bigint NULL DEFAULT NULL COMMENT '封面图片id',
  `create_time` datetime NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
  `update_time` datetime NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
  PRIMARY KEY (`device_id`, `channel_id`) USING BTREE,
  INDEX `idx_gmv_dcc_sort`(`device_id` ASC, `sort_no` ASC, `channel_id` ASC) USING BTREE,
  CONSTRAINT `fk_gmv_dcc_channel` FOREIGN KEY (`device_id`, `channel_id`) REFERENCES `gb28181_device_channel` (`device_id`, `channel_id`) ON DELETE CASCADE ON UPDATE RESTRICT
) ENGINE = InnoDB CHARACTER SET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci COMMENT = '通道业务配置表' ROW_FORMAT = Dynamic;

-- ----------------------------
CREATE TABLE IF NOT EXISTS `gb28181_enum_code` (
  `id` varchar(32) NOT NULL,
  `parent_id` varchar(32) NULL DEFAULT NULL,
  `name` varchar(128) NOT NULL,
  `value_start` varchar(16) NOT NULL,
  `value_end` varchar(16) NOT NULL,
  `remark` varchar(255) NULL DEFAULT NULL,
  `seq` int NULL DEFAULT 0,
  `status` tinyint UNSIGNED NOT NULL DEFAULT 1,
  `created_at` datetime NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (`id`) USING BTREE,
  INDEX `idx_gb28181_enum_code_parent` (`parent_id` ASC) USING BTREE,
  INDEX `idx_gb28181_enum_code_value` (`value_start` ASC, `value_end` ASC, `status` ASC) USING BTREE
) ENGINE = InnoDB CHARACTER SET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci COMMENT = 'GB28181 enum code reference' ROW_FORMAT = Dynamic;

CREATE TABLE IF NOT EXISTS `gb28181_resource_confirmation` (
  `device_id` varchar(20) NOT NULL,
  `resource_id` varchar(32) NOT NULL,
  `resource_kind` varchar(32) NOT NULL,
  `owner_scope` varchar(16) NOT NULL,
  `owner_id` varchar(32) NOT NULL,
  `status` tinyint UNSIGNED NOT NULL DEFAULT 1,
  `suggested_enum_id` varchar(32) NULL DEFAULT NULL,
  `source_parent_id` varchar(32) NULL DEFAULT NULL,
  `confirmed_by` varchar(64) NOT NULL,
  `confirmed_at` datetime NOT NULL DEFAULT CURRENT_TIMESTAMP,
  `remark` varchar(255) NULL DEFAULT NULL,
  `create_time` datetime NOT NULL DEFAULT CURRENT_TIMESTAMP,
  `update_time` datetime NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
  PRIMARY KEY (`device_id`, `resource_id`) USING BTREE,
  INDEX `idx_gb28181_resource_confirmation_owner` (`device_id` ASC, `owner_scope` ASC, `owner_id` ASC, `status` ASC) USING BTREE,
  INDEX `idx_gb28181_resource_confirmation_kind` (`device_id` ASC, `resource_kind` ASC, `status` ASC) USING BTREE
) ENGINE = InnoDB CHARACTER SET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci COMMENT = 'GB28181 resource manual classification override' ROW_FORMAT = Dynamic;

-- Table structure for gb28181_device_ptz_preset
-- ----------------------------
CREATE TABLE IF NOT EXISTS `gb28181_device_ptz_preset`  (
  `id` bigint NOT NULL AUTO_INCREMENT COMMENT '主键',
  `device_id` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '设备id',
  `channel_id` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '视频通道id',
  `preset_no` int NOT NULL COMMENT '预置点编号，GB28181 PTZ Preset 指令使用',
  `preset_name` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '预置点名称',
  `enabled` tinyint UNSIGNED NOT NULL DEFAULT 1 COMMENT '是否启用：0-禁用，1-启用',
  `sort_no` int NULL DEFAULT 0 COMMENT '排序号',
  `remark` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '备注',
  `create_time` datetime NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
  `update_time` datetime NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
  PRIMARY KEY (`id`) USING BTREE,
  UNIQUE INDEX `uk_gmv_ptz_preset`(`device_id` ASC, `channel_id` ASC, `preset_no` ASC) USING BTREE,
  INDEX `idx_gmv_ptz_preset_channel`(`device_id` ASC, `channel_id` ASC, `enabled` ASC, `sort_no` ASC) USING BTREE,
  CONSTRAINT `fk_gmv_ptz_preset_channel` FOREIGN KEY (`device_id`, `channel_id`) REFERENCES `gb28181_device_channel` (`device_id`, `channel_id`) ON DELETE CASCADE ON UPDATE RESTRICT
) ENGINE = InnoDB AUTO_INCREMENT = 1 CHARACTER SET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci COMMENT = '云台预置点配置表' ROW_FORMAT = Dynamic;

-- ----------------------------
-- Table structure for gb28181_file_info
-- ----------------------------
CREATE TABLE IF NOT EXISTS `gb28181_file_info`  (
  `id` bigint NOT NULL AUTO_INCREMENT,
  `device_id` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '设备id',
  `channel_id` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '通道id',
  `biz_time` datetime NULL DEFAULT NULL COMMENT '生成时间',
  `biz_id` varchar(128) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '业务id',
  `file_type` int NULL DEFAULT NULL COMMENT '文件类型：0-图片，1-视频，2-音频，3-视音频，4-其他',
  `file_size` bigint UNSIGNED NULL DEFAULT NULL COMMENT '文件大小BYTE',
  `file_name` varchar(128) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '文件名称',
  `file_format` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '文件格式',
  `dir_path` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '(相对)存储路径',
  `abs_path` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '绝对路径',
  `note` varchar(128) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '注释',
  `is_del` int NULL DEFAULT 0 COMMENT '是否删除;1-是，0-否；默认0',
  `create_time` datetime NULL DEFAULT NULL COMMENT '创建时间',
  PRIMARY KEY (`id`) USING BTREE,
  INDEX `dc_index`(`device_id` ASC, `channel_id` ASC) USING BTREE,
  INDEX `idx_device_channel_id`(`device_id` ASC, `channel_id` ASC, `id` DESC) USING BTREE
) ENGINE = InnoDB AUTO_INCREMENT = 16866 CHARACTER SET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci COMMENT = '文件信息' ROW_FORMAT = Dynamic;

-- ----------------------------
-- Table structure for gb28181_oauth
-- ----------------------------
CREATE TABLE IF NOT EXISTS `gb28181_oauth`  (
  `device_id` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '中心8行业2类型3网络1序号6',
  `domain_id` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL DEFAULT '34020000002000000001' COMMENT '设备域id',
  `domain` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '设备域',
  `longitude` decimal(12, 8) NULL DEFAULT NULL COMMENT '经度',
  `latitude` decimal(12, 8) NULL DEFAULT NULL COMMENT '维度',
  `address` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '地址',
  `pwd` varchar(120) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '密码',
  `pwd_check` tinyint UNSIGNED NULL DEFAULT NULL COMMENT '0-不校验，1-检查',
  `alias` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '别名',
  `status` tinyint UNSIGNED NULL DEFAULT NULL COMMENT '0-禁用，1-启用',
  `heartbeat_sec` tinyint UNSIGNED NULL DEFAULT NULL COMMENT '心跳间隔：秒',
  `del` tinyint UNSIGNED NULL DEFAULT NULL COMMENT '0-未删除，1-已删除',
  `create_time` datetime NULL DEFAULT NULL COMMENT '创建时间',
  `tenant_id` int NULL DEFAULT NULL COMMENT '租户id',
  `sys_org_code` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '机构编码',
  `create_by` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL,
  `update_by` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL,
  `update_time` datetime NULL DEFAULT NULL,
  PRIMARY KEY (`device_id`) USING BTREE,
  UNIQUE INDEX `device_id`(`device_id` ASC) USING BTREE
) ENGINE = InnoDB CHARACTER SET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci COMMENT = '认证表' ROW_FORMAT = Dynamic;

-- ----------------------------
-- Table structure for gb28181_record
-- ----------------------------
CREATE TABLE IF NOT EXISTS `gb28181_record`  (
  `biz_id` varchar(128) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '业务id',
  `device_id` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '设备编号',
  `channel_id` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '通道编号',
  `user_id` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '用户id',
  `st` datetime NULL DEFAULT NULL COMMENT '录像开始时间',
  `et` datetime NULL DEFAULT NULL COMMENT '录像结束时间',
  `speed` tinyint UNSIGNED NULL DEFAULT NULL COMMENT '倍速',
  `ct` datetime NULL DEFAULT NULL COMMENT '创建时间',
  `state` tinyint UNSIGNED NULL DEFAULT NULL COMMENT '录制状态：0=进行，1=完成，2=录制部分，3=失败',
  `lt` datetime NULL DEFAULT NULL COMMENT '最后更新时间',
  `stream_app_name` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '流媒体名称',
  PRIMARY KEY (`biz_id`) USING BTREE
) ENGINE = InnoDB CHARACTER SET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci COMMENT = '云端录像' ROW_FORMAT = Dynamic;

-- ----------------------------
-- Table structure for gb28181_sip_dialog_session
-- ----------------------------
CREATE TABLE IF NOT EXISTS `gb28181_sip_dialog_session`  (
  `stream_id` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `device_id` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `channel_id` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `session_type` varchar(16) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT 'LIVE/PLAYBACK/DOWNLOAD/TALK',
  `signal_node_id` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `media_node_id` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `ssrc` varchar(16) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL,
  `call_id` varchar(128) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `local_uri` varchar(256) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `remote_uri` varchar(256) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `local_tag` varchar(128) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `remote_tag` varchar(128) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL,
  `local_cseq` bigint NOT NULL DEFAULT 1,
  `remote_cseq` bigint NULL DEFAULT NULL,
  `playback_id` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL,
  `playback_start_sec` bigint NULL DEFAULT NULL,
  `playback_end_sec` bigint NULL DEFAULT NULL,
  `playback_generation` bigint NULL DEFAULT NULL,
  `mansrtsp_cseq` bigint NULL DEFAULT NULL,
  `acknowledged_position_sec` bigint NULL DEFAULT NULL,
  `desired_rate_milli` bigint NULL DEFAULT NULL,
  `acknowledged_rate_milli` bigint NULL DEFAULT NULL,
  `last_control_operation_id` varchar(128) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL,
  `contact_uri` varchar(256) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL,
  `route_set` text CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL,
  `local_sip_addr` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `remote_sip_addr` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `transport` varchar(8) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT 'UDP/TCP/TLS',
  `state` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT 'INVITING/ESTABLISHED/TERMINATING/TERMINATED/ORPHAN',
  `established_at` datetime(3) NULL DEFAULT NULL,
  `last_seen_at` datetime(3) NOT NULL,
  `expire_at` datetime(3) NOT NULL,
  `version` bigint NOT NULL DEFAULT 0,
  `created_at` datetime(3) NOT NULL,
  `updated_at` datetime(3) NOT NULL,
  PRIMARY KEY (`stream_id`) USING BTREE,
  INDEX `idx_gmv_sip_dialog_device_state`(`device_id` ASC, `state` ASC) USING BTREE,
  INDEX `idx_gmv_sip_dialog_call_id`(`call_id` ASC) USING BTREE,
  INDEX `idx_gmv_sip_dialog_owner_state_expire`(`signal_node_id` ASC, `state` ASC, `expire_at` ASC) USING BTREE,
  INDEX `idx_gmv_sip_dialog_owner_ssrc_state_expire`(`signal_node_id` ASC, `ssrc` ASC, `state` ASC, `expire_at` ASC) USING BTREE,
  INDEX `idx_gmv_sip_dialog_owner`(`signal_node_id` ASC, `state` ASC, `stream_id` ASC) USING BTREE,
  INDEX `idx_gmv_sip_dialog_ssrc`(`signal_node_id` ASC, `media_node_id` ASC, `ssrc` ASC, `state` ASC, `expire_at` ASC) USING BTREE
) ENGINE = InnoDB CHARACTER SET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci ROW_FORMAT = Dynamic;
