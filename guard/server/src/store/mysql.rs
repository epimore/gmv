use base_db::sqlx::MySqlPool;

use crate::core::{GuardError, GuardResult};
use crate::store::migration::MYSQL_MIGRATIONS;
use crate::store::model::{
    EventRecord, GbChannelImageRecord, GbChannelImageRow, GbChannelRecord, GbChannelRow,
    GbDeviceRecord, GbDeviceRow, GmvRecordInsert, MediaFileInsert, OutboxRecord, OutboxRow,
    RecordFileInsert, gb_channel_from_row, gb_channel_image_from_row, gb_device_from_row,
    outbox_from_row,
};

#[derive(Debug, Clone)]
pub struct MysqlStore {
    pool: MySqlPool,
}

impl MysqlStore {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }

    pub async fn migrate(&self) -> GuardResult<()> {
        base_db::migration::run_mysql_migrations(&self.pool, MYSQL_MIGRATIONS)
            .await
            .map_err(database_error)
    }

    pub async fn insert_event_with_outbox(
        &self,
        event: &EventRecord,
        records: &[OutboxRecord],
    ) -> GuardResult<bool> {
        validate_records(event, records)?;
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let existing = base_db::sqlx::query_scalar::<_, String>(
            "SELECT event_id FROM guard_event WHERE event_id = ?",
        )
        .bind(&event.event_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?;
        if existing.is_some() {
            tx.rollback().await.map_err(database_error)?;
            return Ok(false);
        }
        base_db::sqlx::query(
            "INSERT INTO guard_event(event_id, topic, priority, payload) VALUES (?, ?, ?, ?)",
        )
        .bind(&event.event_id)
        .bind(&event.topic)
        .bind(i64::from(event.priority))
        .bind(&event.payload)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        for record in records {
            insert_outbox_mysql(&mut tx, record).await?;
        }
        tx.commit().await.map_err(database_error)?;
        Ok(true)
    }

    pub async fn due_outbox(&self, now_ms: i64, limit: usize) -> GuardResult<Vec<OutboxRecord>> {
        let rows = base_db::sqlx::query_as::<_, OutboxRow>("SELECT outbox_id,event_id,destination_kind,destination,payload,state,attempts,next_attempt_at_ms,last_error,created_at_ms,updated_at_ms FROM guard_outbox WHERE state IN ('PENDING','RETRY_WAIT') AND next_attempt_at_ms <= ? ORDER BY next_attempt_at_ms,outbox_id LIMIT ?")
            .bind(now_ms).bind(i64::try_from(limit).unwrap_or(i64::MAX)).fetch_all(&self.pool).await.map_err(database_error)?;
        rows.into_iter().map(outbox_from_row).collect()
    }

    pub async fn update_outbox(&self, record: &OutboxRecord) -> GuardResult<()> {
        let result = base_db::sqlx::query("UPDATE guard_outbox SET state=?,attempts=?,next_attempt_at_ms=?,last_error=?,updated_at_ms=? WHERE outbox_id=?")
            .bind(record.state.as_str()).bind(i64::from(record.attempts)).bind(record.next_attempt_at_ms)
            .bind(&record.last_error).bind(record.updated_at_ms).bind(&record.outbox_id)
            .execute(&self.pool).await.map_err(database_error)?;
        if result.rows_affected() == 0 {
            return Err(GuardError::NotFound(format!("outbox {}", record.outbox_id)));
        }
        Ok(())
    }

    pub async fn insert_outbox_records(&self, records: &[OutboxRecord]) -> GuardResult<()> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        for record in records {
            insert_outbox_mysql(&mut tx, record).await?;
        }
        tx.commit().await.map_err(database_error)?;
        Ok(())
    }

    pub async fn running_record_exists(
        &self,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<bool> {
        let row: Option<(i32,)> = base_db::sqlx::query_as(
            "SELECT 1 FROM GMV_RECORD WHERE STATE=0 AND DEVICE_ID=? AND CHANNEL_ID=? LIMIT 1",
        )
        .bind(device_id)
        .bind(channel_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?;
        Ok(row.is_some())
    }

    pub async fn insert_record(&self, record: &GmvRecordInsert) -> GuardResult<()> {
        base_db::sqlx::query("INSERT INTO GMV_RECORD(BIZ_ID,DEVICE_ID,CHANNEL_ID,USER_ID,ST,ET,SPEED,CT,STATE,LT,STREAM_APP_NAME) VALUES (?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&record.biz_id)
            .bind(&record.device_id)
            .bind(&record.channel_id)
            .bind(&record.user_id)
            .bind(&record.st)
            .bind(&record.et)
            .bind(i64::from(record.speed))
            .bind(&record.ct)
            .bind(i64::from(record.state))
            .bind(&record.lt)
            .bind(&record.stream_app_name)
            .execute(&self.pool)
            .await
            .map_err(database_error)?;
        Ok(())
    }

    pub async fn finish_record(&self, file: &RecordFileInsert) -> GuardResult<bool> {
        let row: Option<(String, String, String, String)> = base_db::sqlx::query_as(
            "SELECT DEVICE_ID,CHANNEL_ID,ST,ET FROM GMV_RECORD WHERE BIZ_ID=?",
        )
        .bind(&file.biz_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?;
        let Some((device_id, channel_id, st, et)) = row else {
            return Ok(false);
        };
        let state = record_state(&st, &et, file.file_size, file.record_duration_sec);
        base_db::sqlx::query("UPDATE GMV_RECORD SET STATE=?,LT=? WHERE BIZ_ID=?")
            .bind(i64::from(state))
            .bind(&file.now)
            .bind(&file.biz_id)
            .execute(&self.pool)
            .await
            .map_err(database_error)?;
        self.insert_media_file(&MediaFileInsert {
            id: crate::media::next_file_id(),
            device_id,
            channel_id,
            biz_time: file.now.clone(),
            biz_id: file.biz_id.clone(),
            file_type: 1,
            file_size: file.file_size,
            file_name: file.biz_id.clone(),
            file_format: file.file_format.clone(),
            dir_path: file.dir_path.clone(),
            abs_path: file.abs_path.clone(),
            note: None,
            is_del: 0,
            create_time: file.now.clone(),
        })
        .await?;
        Ok(true)
    }

    pub async fn insert_media_file(&self, file: &MediaFileInsert) -> GuardResult<()> {
        let file_size = i64::try_from(file.file_size).unwrap_or(i64::MAX);
        base_db::sqlx::query("INSERT INTO GMV_FILE_INFO(ID,DEVICE_ID,CHANNEL_ID,BIZ_TIME,BIZ_ID,FILE_TYPE,FILE_SIZE,FILE_NAME,FILE_FORMAT,DIR_PATH,ABS_PATH,NOTE,IS_DEL,CREATE_TIME) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(file.id)
            .bind(&file.device_id)
            .bind(&file.channel_id)
            .bind(&file.biz_time)
            .bind(&file.biz_id)
            .bind(file.file_type)
            .bind(file_size)
            .bind(&file.file_name)
            .bind(&file.file_format)
            .bind(&file.dir_path)
            .bind(&file.abs_path)
            .bind(&file.note)
            .bind(file.is_del)
            .bind(&file.create_time)
            .execute(&self.pool)
            .await
            .map_err(database_error)?;
        Ok(())
    }

    pub async fn list_gb_devices(&self) -> GuardResult<Vec<GbDeviceRecord>> {
        let rows = base_db::sqlx::query_as::<_, GbDeviceRow>(
            "SELECT DEVICE_ID AS device_id,'' AS session_node_id,DOMAIN_ID AS domain_id,DOMAIN AS domain,longitude,latitude,address,PWD AS pwd,COALESCE(PWD_CHECK,1) AS pwd_check,ALIAS AS alias,COALESCE(STATUS,1) AS status,COALESCE(HEARTBEAT_SEC,60) AS heartbeat_sec,COALESCE(DEL,0) AS del,CREATE_TIME AS create_time,tenant_id,sys_org_code,create_by,update_by,update_time FROM GMV_OAUTH WHERE COALESCE(DEL,0)=0 ORDER BY CREATE_TIME DESC,DEVICE_ID",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        Ok(rows.into_iter().map(gb_device_from_row).collect())
    }

    pub async fn get_gb_device(&self, device_id: &str) -> GuardResult<Option<GbDeviceRecord>> {
        let row = base_db::sqlx::query_as::<_, GbDeviceRow>(
            "SELECT DEVICE_ID AS device_id,'' AS session_node_id,DOMAIN_ID AS domain_id,DOMAIN AS domain,longitude,latitude,address,PWD AS pwd,COALESCE(PWD_CHECK,1) AS pwd_check,ALIAS AS alias,COALESCE(STATUS,1) AS status,COALESCE(HEARTBEAT_SEC,60) AS heartbeat_sec,COALESCE(DEL,0) AS del,CREATE_TIME AS create_time,tenant_id,sys_org_code,create_by,update_by,update_time FROM GMV_OAUTH WHERE DEVICE_ID=?",
        )
        .bind(device_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?;
        Ok(row.map(gb_device_from_row))
    }

    pub async fn upsert_gb_device(&self, device: &GbDeviceRecord) -> GuardResult<()> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        base_db::sqlx::query("DELETE FROM GMV_OAUTH WHERE DEVICE_ID=?")
            .bind(&device.device_id)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        base_db::sqlx::query(
            "INSERT INTO GMV_OAUTH(DEVICE_ID,DOMAIN_ID,DOMAIN,longitude,latitude,address,PWD,PWD_CHECK,ALIAS,STATUS,HEARTBEAT_SEC,DEL,CREATE_TIME,tenant_id,sys_org_code,create_by,update_by,update_time) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&device.device_id)
        .bind(&device.domain_id)
        .bind(&device.domain)
        .bind(&device.longitude)
        .bind(&device.latitude)
        .bind(&device.address)
        .bind(&device.pwd)
        .bind(device.pwd_check)
        .bind(&device.alias)
        .bind(device.status)
        .bind(device.heartbeat_sec)
        .bind(device.del)
        .bind(&device.create_time)
        .bind(&device.tenant_id)
        .bind(&device.sys_org_code)
        .bind(&device.create_by)
        .bind(&device.update_by)
        .bind(&device.update_time)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(())
    }

    pub async fn delete_gb_device(&self, device_id: &str) -> GuardResult<bool> {
        let result = base_db::sqlx::query(
            "UPDATE GMV_OAUTH SET STATUS=0,DEL=1,update_time=CURRENT_TIMESTAMP WHERE DEVICE_ID=? AND COALESCE(DEL,0)=0",
        )
        .bind(device_id)
        .execute(&self.pool)
        .await
        .map_err(database_error)?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn list_gb_channels(&self, device_id: &str) -> GuardResult<Vec<GbChannelRecord>> {
        let rows = base_db::sqlx::query_as::<_, GbChannelRow>("SELECT device_id,channel_id,name,manufacturer,model,owner,status,civil_code,address,parent_id,ip_address,port,longitude,latitude,ptz_type,alias_name,pic_url,snapshot,over_pic_id,ptz_enable,talk_enable,audio_enable,record_enable,playback_enable,alarm_enable,biz_enable,sort_no,created_at_ms,updated_at_ms FROM gmv_gb28181_channel WHERE device_id=? ORDER BY sort_no,channel_id")
            .bind(device_id)
            .fetch_all(&self.pool)
            .await
            .map_err(database_error)?;
        Ok(rows.into_iter().map(gb_channel_from_row).collect())
    }

    pub async fn get_gb_channel(
        &self,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<Option<GbChannelRecord>> {
        let row = base_db::sqlx::query_as::<_, GbChannelRow>("SELECT device_id,channel_id,name,manufacturer,model,owner,status,civil_code,address,parent_id,ip_address,port,longitude,latitude,ptz_type,alias_name,pic_url,snapshot,over_pic_id,ptz_enable,talk_enable,audio_enable,record_enable,playback_enable,alarm_enable,biz_enable,sort_no,created_at_ms,updated_at_ms FROM gmv_gb28181_channel WHERE device_id=? AND channel_id=?")
            .bind(device_id)
            .bind(channel_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(database_error)?;
        Ok(row.map(gb_channel_from_row))
    }

    pub async fn upsert_gb_channel(&self, channel: &GbChannelRecord) -> GuardResult<()> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        base_db::sqlx::query("DELETE FROM gmv_gb28181_channel WHERE device_id=? AND channel_id=?")
            .bind(&channel.device_id)
            .bind(&channel.channel_id)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        base_db::sqlx::query("INSERT INTO gmv_gb28181_channel(device_id,channel_id,name,manufacturer,model,owner,status,civil_code,address,parent_id,ip_address,port,longitude,latitude,ptz_type,alias_name,pic_url,snapshot,over_pic_id,ptz_enable,talk_enable,audio_enable,record_enable,playback_enable,alarm_enable,biz_enable,sort_no,created_at_ms,updated_at_ms) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&channel.device_id)
            .bind(&channel.channel_id)
            .bind(&channel.name)
            .bind(&channel.manufacturer)
            .bind(&channel.model)
            .bind(&channel.owner)
            .bind(&channel.status)
            .bind(&channel.civil_code)
            .bind(&channel.address)
            .bind(&channel.parent_id)
            .bind(&channel.ip_address)
            .bind(channel.port)
            .bind(&channel.longitude)
            .bind(&channel.latitude)
            .bind(&channel.ptz_type)
            .bind(&channel.alias_name)
            .bind(&channel.pic_url)
            .bind(channel.snapshot)
            .bind(&channel.over_pic_id)
            .bind(channel.ptz_enable)
            .bind(channel.talk_enable)
            .bind(channel.audio_enable)
            .bind(channel.record_enable)
            .bind(channel.playback_enable)
            .bind(channel.alarm_enable)
            .bind(channel.biz_enable)
            .bind(channel.sort_no)
            .bind(channel.created_at_ms)
            .bind(channel.updated_at_ms)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(())
    }

    pub async fn delete_gb_channel(&self, device_id: &str, channel_id: &str) -> GuardResult<bool> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        base_db::sqlx::query(
            "DELETE FROM gmv_gb28181_channel_image WHERE device_id=? AND channel_id=?",
        )
        .bind(device_id)
        .bind(channel_id)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        let result = base_db::sqlx::query(
            "DELETE FROM gmv_gb28181_channel WHERE device_id=? AND channel_id=?",
        )
        .bind(device_id)
        .bind(channel_id)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn list_gb_channel_images(
        &self,
        device_id: &str,
        channel_id: &str,
    ) -> GuardResult<Vec<GbChannelImageRecord>> {
        let rows = base_db::sqlx::query_as::<_, GbChannelImageRow>("SELECT image_id,device_id,channel_id,image_url,created_at_ms FROM gmv_gb28181_channel_image WHERE device_id=? AND channel_id=? ORDER BY created_at_ms DESC,image_id")
            .bind(device_id)
            .bind(channel_id)
            .fetch_all(&self.pool)
            .await
            .map_err(database_error)?;
        Ok(rows.into_iter().map(gb_channel_image_from_row).collect())
    }

    pub async fn insert_gb_channel_image(&self, image: &GbChannelImageRecord) -> GuardResult<()> {
        base_db::sqlx::query("INSERT INTO gmv_gb28181_channel_image(image_id,device_id,channel_id,image_url,created_at_ms) VALUES (?,?,?,?,?)")
            .bind(&image.image_id)
            .bind(&image.device_id)
            .bind(&image.channel_id)
            .bind(&image.image_url)
            .bind(image.created_at_ms)
            .execute(&self.pool)
            .await
            .map_err(database_error)?;
        Ok(())
    }

    pub async fn outbox_records(&self, limit: usize) -> GuardResult<Vec<OutboxRecord>> {
        let rows = base_db::sqlx::query_as::<_, OutboxRow>(
            "SELECT outbox_id,event_id,destination_kind,destination,payload,state,attempts,next_attempt_at_ms,last_error,created_at_ms,updated_at_ms FROM guard_outbox ORDER BY created_at_ms DESC,outbox_id LIMIT ?",
        )
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        rows.into_iter().map(outbox_from_row).collect()
    }

    pub async fn claim_command(
        &self,
        command_id: &str,
        expires_at_ms: i64,
        now_ms: i64,
    ) -> GuardResult<bool> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        base_db::sqlx::query("DELETE FROM guard_command WHERE expires_at_ms < ?")
            .bind(now_ms)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        let existing = base_db::sqlx::query_scalar::<_, String>(
            "SELECT command_id FROM guard_command WHERE command_id = ?",
        )
        .bind(command_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?;
        if existing.is_some() {
            tx.rollback().await.map_err(database_error)?;
            return Ok(false);
        }
        base_db::sqlx::query(
            "INSERT INTO guard_command(command_id,expires_at_ms,created_at_ms) VALUES (?,?,?)",
        )
        .bind(command_id)
        .bind(expires_at_ms)
        .bind(now_ms)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(true)
    }

    pub async fn recover_stale_sending(
        &self,
        stale_before_ms: i64,
        now_ms: i64,
    ) -> GuardResult<u64> {
        let result = base_db::sqlx::query(
            "UPDATE guard_outbox SET state='RETRY_WAIT',next_attempt_at_ms=?,last_error='delivery interrupted before completion',updated_at_ms=? WHERE state='SENDING' AND updated_at_ms <= ?",
        )
        .bind(now_ms)
        .bind(now_ms)
        .bind(stale_before_ms)
        .execute(&self.pool)
        .await
        .map_err(database_error)?;
        Ok(result.rows_affected())
    }

    pub async fn retry_dead_outbox(
        &self,
        outbox_id: &str,
        now_ms: i64,
    ) -> GuardResult<OutboxRecord> {
        let result = base_db::sqlx::query("UPDATE guard_outbox SET state='PENDING',attempts=0,next_attempt_at_ms=?,last_error=NULL,updated_at_ms=? WHERE outbox_id=? AND state='DEAD'")
            .bind(now_ms).bind(now_ms).bind(outbox_id).execute(&self.pool).await.map_err(database_error)?;
        if result.rows_affected() == 0 {
            return Err(GuardError::Conflict(format!(
                "outbox {outbox_id} is not dead"
            )));
        }
        self.get_outbox(outbox_id).await
    }

    pub async fn get_outbox(&self, outbox_id: &str) -> GuardResult<OutboxRecord> {
        let row = base_db::sqlx::query_as::<_, OutboxRow>("SELECT outbox_id,event_id,destination_kind,destination,payload,state,attempts,next_attempt_at_ms,last_error,created_at_ms,updated_at_ms FROM guard_outbox WHERE outbox_id=?")
            .bind(outbox_id).fetch_optional(&self.pool).await.map_err(database_error)?
            .ok_or_else(|| GuardError::NotFound(format!("outbox {outbox_id}")))?;
        outbox_from_row(row)
    }
    pub async fn list_user_profiles(&self) -> GuardResult<Vec<crate::auth::UserProfile>> {
        let rows = base_db::sqlx::query_as::<_, (String, String, String, i64, i64, i64)>(
            "SELECT username,role,nickname,enabled,created_at_ms,updated_at_ms FROM guard_user ORDER BY username",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        rows.into_iter()
            .map(
                |(username, role, nickname, enabled, created_at_ms, updated_at_ms)| {
                    Ok(crate::auth::UserProfile {
                        username,
                        role: crate::auth::Role::parse(&role)?,
                        nickname,
                        enabled: enabled != 0,
                        created_at_ms,
                        updated_at_ms,
                    })
                },
            )
            .collect()
    }

    pub async fn load_user(&self, username: &str) -> GuardResult<Option<crate::auth::UserAccount>> {
        let row = base_db::sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT username,role,nickname,password_hash FROM guard_user WHERE username=? AND enabled=1",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?;
        row.map(|(username, role, nickname, hash)| {
            Ok(crate::auth::UserAccount::with_nickname(
                username,
                crate::auth::Role::parse(&role)?,
                nickname,
                hash,
            ))
        })
        .transpose()
    }

    pub async fn upsert_user(
        &self,
        username: &str,
        role: crate::auth::Role,
        password_hash: Option<&str>,
        nickname: Option<&str>,
        enabled: bool,
        now_ms: i64,
    ) -> GuardResult<()> {
        if username.trim().is_empty() {
            return Err(GuardError::InvalidConfig(
                "username is required".to_string(),
            ));
        }
        if let Some(hash) = password_hash {
            crate::auth::UserAccount::new(username, role, hash).validate_password_hash()?;
        }
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let existing_nickname = base_db::sqlx::query_scalar::<_, String>(
            "SELECT nickname FROM guard_user WHERE username=?",
        )
        .bind(username)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?;
        let enabled = if enabled { 1_i64 } else { 0_i64 };
        let nickname = nickname
            .map(str::trim)
            .map(str::to_string)
            .or_else(|| existing_nickname.clone())
            .unwrap_or_default();
        match (existing_nickname.is_some(), password_hash) {
            (true, Some(hash)) => {
                base_db::sqlx::query("UPDATE guard_user SET role=?,password_hash=?,nickname=?,enabled=?,updated_at_ms=? WHERE username=?")
                    .bind(role.as_str())
                    .bind(hash)
                    .bind(&nickname)
                    .bind(enabled)
                    .bind(now_ms)
                    .bind(username)
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error)?;
            }
            (true, None) => {
                base_db::sqlx::query(
                    "UPDATE guard_user SET role=?,nickname=?,enabled=?,updated_at_ms=? WHERE username=?",
                )
                .bind(role.as_str())
                .bind(&nickname)
                .bind(enabled)
                .bind(now_ms)
                .bind(username)
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
            }
            (false, Some(hash)) => {
                base_db::sqlx::query("INSERT INTO guard_user(username,role,password_hash,nickname,enabled,created_at_ms,updated_at_ms) VALUES (?,?,?,?,?,?,?)")
                    .bind(username)
                    .bind(role.as_str())
                    .bind(hash)
                    .bind(&nickname)
                    .bind(enabled)
                    .bind(now_ms)
                    .bind(now_ms)
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error)?;
            }
            (false, None) => {
                tx.rollback().await.map_err(database_error)?;
                return Err(GuardError::InvalidConfig(
                    "password is required for new UI users".to_string(),
                ));
            }
        }
        tx.commit().await.map_err(database_error)?;
        Ok(())
    }

    pub async fn load_users(&self) -> GuardResult<Vec<crate::auth::UserAccount>> {
        let rows = base_db::sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT username,role,nickname,password_hash FROM guard_user WHERE enabled=1 ORDER BY username",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        rows.into_iter()
            .map(|(username, role, nickname, hash)| {
                Ok(crate::auth::UserAccount::with_nickname(
                    username,
                    crate::auth::Role::parse(&role)?,
                    nickname,
                    hash,
                ))
            })
            .collect()
    }

    pub async fn revoke_ui_sessions(&self, username: &str) -> GuardResult<()> {
        base_db::sqlx::query("DELETE FROM guard_ui_session WHERE username=?")
            .bind(username)
            .execute(&self.pool)
            .await
            .map_err(database_error)?;
        Ok(())
    }

    pub async fn bootstrap_admin(&self, username: &str, password_hash: &str) -> GuardResult<bool> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let count = base_db::sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM guard_user")
            .fetch_one(&mut *tx)
            .await
            .map_err(database_error)?;
        if count != 0 {
            tx.rollback().await.map_err(database_error)?;
            return Ok(false);
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        base_db::sqlx::query("INSERT INTO guard_user(username,role,password_hash,enabled,created_at_ms,updated_at_ms) VALUES (?,?,?,?,?,?)")
            .bind(username)
            .bind(crate::auth::Role::Admin.as_str())
            .bind(password_hash)
            .bind(1_i64)
            .bind(now_ms)
            .bind(now_ms)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(true)
    }
}

async fn insert_outbox_mysql(
    tx: &mut base_db::sqlx::Transaction<'_, base_db::sqlx::MySql>,
    record: &OutboxRecord,
) -> GuardResult<()> {
    base_db::sqlx::query("INSERT INTO guard_outbox(outbox_id,event_id,destination_kind,destination,payload,state,attempts,next_attempt_at_ms,last_error,created_at_ms,updated_at_ms) VALUES (?,?,?,?,?,?,?,?,?,?,?)")
        .bind(&record.outbox_id).bind(&record.event_id).bind(record.destination_kind.as_str()).bind(&record.destination)
        .bind(&record.payload).bind(record.state.as_str()).bind(i64::from(record.attempts)).bind(record.next_attempt_at_ms)
        .bind(&record.last_error).bind(record.created_at_ms).bind(record.updated_at_ms)
        .execute(&mut **tx).await.map_err(database_error)?;
    Ok(())
}

fn validate_records(event: &EventRecord, records: &[OutboxRecord]) -> GuardResult<()> {
    if records.iter().any(|record| {
        record.event_id != event.event_id
            || record.outbox_id.is_empty()
            || record.destination.is_empty()
    }) {
        return Err(GuardError::InvalidConfig(
            "outbox records must match event and have ids/destinations".to_string(),
        ));
    }
    Ok(())
}

fn database_error(error: impl std::fmt::Display) -> GuardError {
    GuardError::Conflict(format!("outbox database operation failed: {error}"))
}

fn record_state(st: &str, et: &str, file_size: u64, record_duration_sec: u64) -> i32 {
    if file_size == 0 || record_duration_sec == 0 {
        return 3;
    }
    let total_secs = parse_datetime(et)
        .zip(parse_datetime(st))
        .map(|(end, start)| end.signed_duration_since(start).num_seconds())
        .unwrap_or_default();
    if total_secs <= 0 {
        return 3;
    }
    let per = i64::try_from(record_duration_sec)
        .unwrap_or(i64::MAX)
        .saturating_mul(1000)
        / total_secs;
    if per > 98 { 1 } else { 2 }
}

fn parse_datetime(value: &str) -> Option<base::chrono::NaiveDateTime> {
    base::chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").ok()
}
