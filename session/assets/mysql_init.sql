SET NAMES utf8mb4;
SET FOREIGN_KEY_CHECKS = 0;

-- ----------------------------
-- Table structure for C_AREA_CODE
-- ----------------------------
DROP TABLE IF EXISTS `C_AREA_CODE`;
CREATE TABLE `C_AREA_CODE`  (
                                `code` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci NOT NULL COMMENT '行政区划代码',
                                `name` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci NULL DEFAULT NULL COMMENT '行政区划',
                                `name_full` varchar(128) CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci NULL DEFAULT NULL COMMENT '行政区划-全称',
                                `province_code` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci NULL DEFAULT NULL COMMENT '行政区划代码-省',
                                `province_name` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci NULL DEFAULT NULL COMMENT '行政区划-省',
                                `city_code` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci NULL DEFAULT NULL COMMENT '行政区划代码-市',
                                `city_name` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci NULL DEFAULT NULL COMMENT '行政区划-市',
                                `district_code` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci NULL DEFAULT NULL COMMENT '行政区划代码-区/县',
                                `district_name` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci NULL DEFAULT NULL COMMENT '行政区划-区/县',
                                `street_code` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci NULL DEFAULT NULL COMMENT '行政区划代码-乡镇/街道',
                                `street_name` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci NULL DEFAULT NULL COMMENT '行政区划-乡镇/街道',
                                `level` int NULL DEFAULT NULL COMMENT '行政区划级别',
                                PRIMARY KEY (`code`) USING BTREE,
                                INDEX `index_code`(`code` ASC) USING BTREE,
                                INDEX `index_name`(`name` ASC) USING BTREE
) ENGINE = InnoDB CHARACTER SET = utf8mb4 COLLATE = utf8mb4_general_ci ROW_FORMAT = Dynamic;

-- ----------------------------
-- Table structure for C_SEQ_CODE
-- ----------------------------
DROP TABLE IF EXISTS `C_SEQ_CODE`;
CREATE TABLE `C_SEQ_CODE`  (
                               `seq_id` bigint(16) UNSIGNED ZEROFILL NOT NULL AUTO_INCREMENT COMMENT '序列标识',
                               `seq_name` varchar(64) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL COMMENT '序列的名字，唯一',
                               `init_value` bigint UNSIGNED NOT NULL COMMENT '初始值',
                               `current_value` bigint UNSIGNED NOT NULL COMMENT '当前的值',
                               `increment_value` int NOT NULL DEFAULT 1 COMMENT '步长，默认为1',
                               `prefix_code` varchar(64) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NULL DEFAULT NULL COMMENT '前置编码',
                               `code_lenth` int NULL DEFAULT NULL COMMENT '编码长度(不含前置)，中间以0填充',
                               `remark` varchar(256) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NULL DEFAULT NULL COMMENT '备注',
                               PRIMARY KEY (`seq_id`) USING BTREE,
                               UNIQUE INDEX `udx_seq_name`(`seq_name` ASC) USING BTREE
) ENGINE = InnoDB AUTO_INCREMENT = 15 CHARACTER SET = utf8mb3 COLLATE = utf8mb3_general_ci COMMENT = '公共的序列表' ROW_FORMAT = DYNAMIC;

-- ----------------------------
-- Table structure for GMV_DEVICE
-- ----------------------------
DROP TABLE IF EXISTS `GMV_DEVICE`;
CREATE TABLE `GMV_DEVICE`  (
                               `DEVICE_ID` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '设备主键ID',
                               `TRANSPORT` varchar(3) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '网络协议：TCP/UDP',
                               `REGISTER_EXPIRES` int UNSIGNED NULL DEFAULT NULL COMMENT '注册有效期',
                               `REGISTER_TIME` datetime NULL DEFAULT NULL COMMENT '最近注册时间',
                               `LOCAL_ADDR` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备本地地址',
                               `CONTACT_URI` varchar(128) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '请求地址',
                               `ENABLE_LR` tinyint UNSIGNED NULL DEFAULT NULL COMMENT '1-是，0-否',
                               `DEVICE_TYPE` varchar(16) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备类型IPC/NVR/DVR...',
                               `MANUFACTURER` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '厂家名称',
                               `MODEL` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备型号',
                               `FIRMWARE` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '固件版本',
                               `MAX_CAMERA` smallint UNSIGNED NULL DEFAULT NULL COMMENT '最大相机数',
                               `STATUS` tinyint UNSIGNED NULL DEFAULT NULL COMMENT '0-离线，1-在线',
                               `GB_VERSION` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT '2.0' COMMENT '国标版本',
                               `LAST_UPDATE_TIME` datetime NULL DEFAULT NULL ON UPDATE CURRENT_TIMESTAMP COMMENT '最后更新时间',
                               `CREATE_TIME` datetime NULL DEFAULT NULL,
                               `tenant_id` int NULL DEFAULT NULL COMMENT '租户ID',
                               `sys_org_code` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '机构编码',
                               `create_by` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL,
                               `update_by` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL,
                               `update_time` datetime NULL DEFAULT NULL,
                               PRIMARY KEY (`DEVICE_ID`) USING BTREE
) ENGINE = InnoDB CHARACTER SET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci COMMENT = '设备主表' ROW_FORMAT = Dynamic;

-- ----------------------------
-- Table structure for GMV_DEVICE_CHANNEL
-- ----------------------------
DROP TABLE IF EXISTS `GMV_DEVICE_CHANNEL`;
CREATE TABLE `GMV_DEVICE_CHANNEL`  (
                                       `DEVICE_ID` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '设备ID',
                                       `CHANNEL_ID` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '通道ID',
                                       `NAME` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备名称',
                                       `MANUFACTURER` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备厂商',
                                       `MODEL` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备型号',
                                       `OWNER` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备归属',
                                       `STATUS` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT 'ON' COMMENT '设备状态ON默认/OFF/STATUS1/ONLINE/OFFLINE....',
                                       `CIVIL_CODE` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '行政区域',
                                       `ADDRESS` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '安装地址',
                                       `PARENTAL` char(1) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '是否有子设备 1 有， 0 没有',
                                       `BLOCK` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '警区',
                                       `PARENT_ID` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '父设备/区域/系统 ID',
                                       `IP_ADDRESS` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备/区域/系统 IP 地址',
                                       `PORT` int NULL DEFAULT NULL COMMENT '设备/区域/系统端口',
                                       `PASSWORD` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备口令',
                                       `LONGITUDE` decimal(12, 6) NULL DEFAULT NULL COMMENT '经度',
                                       `LATITUDE` decimal(12, 6) NULL DEFAULT NULL COMMENT '纬度',
                                       `PTZ_TYPE` char(1) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '摄像机类型扩展，标识摄像机类型： 1-球机； 2-半球； 3-固定枪机；4-遥控枪机,5遥控半球，6多目设备拼接通道，7多目设备分割通道。',
                                       `SUPPLY_LIGHT_TYPE` char(1) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '摄像机补光属性。 1-无补光、 2-红外补光、 3-白光补光。',
                                       `ALIAS_NAME` varchar(16) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '设备别名',
                                       `SNAPSHOT` int NULL DEFAULT 1 COMMENT '是否启用拍照：0-否，1-是，2-设备不支持；默认1,',
                                       `over_pic_id` bigint NULL DEFAULT NULL COMMENT '封面图片ID',
                                       PRIMARY KEY (`DEVICE_ID`, `CHANNEL_ID`) USING BTREE
) ENGINE = InnoDB CHARACTER SET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci COMMENT = '摄像机通道信息' ROW_FORMAT = Dynamic;

-- ----------------------------
-- Table structure for GMV_DEVICE_SEQ
-- ----------------------------
DROP TABLE IF EXISTS `GMV_DEVICE_SEQ`;
CREATE TABLE `GMV_DEVICE_SEQ`  (
                                   `DOMIN` varchar(10) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT 'SIP设备域',
                                   `SEQ_NO` int NULL DEFAULT NULL COMMENT '序号',
                                   `INFO` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL,
                                   PRIMARY KEY (`DOMIN`) USING BTREE
) ENGINE = InnoDB CHARACTER SET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci ROW_FORMAT = Dynamic;

-- ----------------------------
-- Table structure for GMV_FILE_INFO
-- ----------------------------
DROP TABLE IF EXISTS `GMV_FILE_INFO`;
CREATE TABLE `GMV_FILE_INFO`  (
                                  `ID` bigint NOT NULL AUTO_INCREMENT,
                                  `DEVICE_ID` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '设备ID',
                                  `CHANNEL_ID` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '通道ID',
                                  `BIZ_TIME` datetime NULL DEFAULT NULL COMMENT '生成时间',
                                  `BIZ_ID` varchar(128) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '业务ID',
                                  `FILE_TYPE` int NULL DEFAULT NULL COMMENT '文件类型：0-图片，1-视频，2-音频，3-视音频，4-其他',
                                  `FILE_SIZE` bigint UNSIGNED NULL DEFAULT NULL COMMENT '文件大小BYTE',
                                  `FILE_NAME` varchar(128) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '文件名称',
                                  `FILE_FORMAT` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '文件格式',
                                  `DIR_PATH` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '(相对)存储路径',
                                  `ABS_PATH` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '绝对路径',
                                  `NOTE` varchar(128) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '注释',
                                  `IS_DEL` int NULL DEFAULT 0 COMMENT '是否删除;1-是，0-否；默认0',
                                  `CREATE_TIME` datetime NULL DEFAULT NULL COMMENT '创建时间',
                                  PRIMARY KEY (`ID`) USING BTREE,
                                  INDEX `dc_index`(`DEVICE_ID` ASC, `CHANNEL_ID` ASC) USING BTREE,
                                  INDEX `idx_device_channel_id`(`DEVICE_ID` ASC, `CHANNEL_ID` ASC, `ID` DESC) USING BTREE
) ENGINE = InnoDB AUTO_INCREMENT = 338 CHARACTER SET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci COMMENT = '文件信息' ROW_FORMAT = Dynamic;

-- ----------------------------
-- Table structure for GMV_OAUTH
-- ----------------------------
DROP TABLE IF EXISTS `GMV_OAUTH`;
CREATE TABLE `GMV_OAUTH`  (
                              `DEVICE_ID` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '中心8行业2类型3网络1序号6',
                              `DOMAIN_ID` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL DEFAULT '34020000002000000001' COMMENT '设备域ID',
                              `DOMAIN` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '设备域',
                              `longitude` decimal(12, 8) NULL DEFAULT NULL COMMENT '经度',
                              `latitude` decimal(12, 8) NULL DEFAULT NULL COMMENT '维度',
                              `address` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '地址',
                              `PWD` varchar(120) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '密码',
                              `PWD_CHECK` tinyint UNSIGNED NULL DEFAULT NULL COMMENT '0-不校验，1-检查',
                              `ALIAS` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '别名',
                              `STATUS` tinyint UNSIGNED NULL DEFAULT NULL COMMENT '0-禁用，1-启用',
                              `HEARTBEAT_SEC` tinyint UNSIGNED NULL DEFAULT NULL COMMENT '心跳间隔：秒',
                              `DEL` tinyint UNSIGNED NULL DEFAULT NULL COMMENT '0-未删除，1-已删除',
                              `CREATE_TIME` datetime NULL DEFAULT NULL COMMENT '创建时间',
                              `tenant_id` int NULL DEFAULT NULL COMMENT '租户ID',
                              `sys_org_code` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '机构编码',
                              `create_by` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL,
                              `update_by` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL,
                              `update_time` datetime NULL DEFAULT NULL,
                              PRIMARY KEY (`DEVICE_ID`) USING BTREE,
                              UNIQUE INDEX `DEVICE_ID`(`DEVICE_ID` ASC) USING BTREE
) ENGINE = InnoDB CHARACTER SET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci COMMENT = '认证表' ROW_FORMAT = Dynamic;

-- ----------------------------
-- Table structure for GMV_RECORD
-- ----------------------------
DROP TABLE IF EXISTS `GMV_RECORD`;
CREATE TABLE `GMV_RECORD`  (
                               `BIZ_ID` varchar(128) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '业务ID',
                               `DEVICE_ID` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '设备编号',
                               `CHANNEL_ID` varchar(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL COMMENT '通道编号',
                               `USER_ID` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '用户ID',
                               `ST` datetime NULL DEFAULT NULL COMMENT '录像开始时间',
                               `ET` datetime NULL DEFAULT NULL COMMENT '录像结束时间',
                               `SPEED` tinyint UNSIGNED NULL DEFAULT NULL COMMENT '倍速',
                               `CT` datetime NULL DEFAULT NULL COMMENT '创建时间',
                               `STATE` tinyint UNSIGNED NULL DEFAULT NULL COMMENT '录制状态：0=进行，1=完成，2=录制部分，3=失败',
                               `LT` datetime NULL DEFAULT NULL COMMENT '最后更新时间',
                               `STREAM_APP_NAME` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NULL DEFAULT NULL COMMENT '流媒体名称',
                               PRIMARY KEY (`BIZ_ID`) USING BTREE
) ENGINE = InnoDB CHARACTER SET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci COMMENT = '云端录像' ROW_FORMAT = Dynamic;

-- ----------------------------
-- Function structure for f_get_seq_code
-- ----------------------------
DROP FUNCTION IF EXISTS `f_get_seq_code`;
delimiter ;;
CREATE FUNCTION `f_get_seq_code`(p_seq_name VARCHAR(64))
    RETURNS varchar(128) CHARSET utf8mb4
  MODIFIES SQL DATA
  DETERMINISTIC
BEGIN
UPDATE C_SEQ_CODE
SET current_value = @cur_val := current_value + increment_value
WHERE seq_name = p_seq_name;

RETURN CONCAT_WS('',
                 (SELECT prefix_code FROM C_SEQ_CODE WHERE seq_name = p_seq_name),
                 IFNULL(LPAD(@cur_val,
                             (SELECT code_lenth FROM C_SEQ_CODE WHERE seq_name = p_seq_name),
                             '0'),
                        @cur_val)
       );
END
;;
delimiter ;

SET FOREIGN_KEY_CHECKS = 1;