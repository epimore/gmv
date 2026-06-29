use std::sync::atomic::{AtomicU16, Ordering};

use crate::storage::db;
use base::chrono::Local;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use base_db::sqlx;

const SSRC_SEQUENCE_MAX: u16 = 9_999;
const SSRC_CODE_LENGTH: i32 = 4;

#[cfg(test)]
static TEST_REALTIME_SEQUENCE: AtomicU16 = AtomicU16::new(1);
#[cfg(test)]
static TEST_HISTORY_SEQUENCE: AtomicU16 = AtomicU16::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SsrcKind {
    Realtime,
    History,
}

impl SsrcKind {
    fn marker(self) -> char {
        match self {
            Self::Realtime => '0',
            Self::History => '1',
        }
    }

    fn remark(self) -> &'static str {
        match self {
            Self::Realtime => "SIP域实时SSRC",
            Self::History => "SIP域历史SSRC",
        }
    }
}

pub struct SsrcSequence;

impl SsrcSequence {
    pub async fn initialize(domain_id: &str) -> GlobalResult<()> {
        let realtime = prefix(domain_id, SsrcKind::Realtime)?;
        let history = prefix(domain_id, SsrcKind::History)?;

        #[cfg(test)]
        if crate::storage::entity::test_storage_enabled() {
            let _ = (realtime, history);
            return Ok(());
        }

        for (seq_name, kind) in [(realtime, SsrcKind::Realtime), (history, SsrcKind::History)] {
            let sql = match db::backend() {
                db::SessionDatabaseBackend::Mysql => {
                    "INSERT IGNORE INTO GB28181_SEQ_CODE (seq_name,init_value,current_value,increment_value,prefix_code,code_lenth,remark,create_date)VALUES (?,1,1,1,?,4,?,?)"
                }
                db::SessionDatabaseBackend::Sqlite => {
                    "INSERT INTO GB28181_SEQ_CODE (seq_name,init_value,current_value,increment_value,prefix_code,code_lenth,remark,create_date) VALUES (?,1,1,1,?,4,?,?) ON CONFLICT(seq_name) DO NOTHING"
                }
            };
            db::execute!(
                sql,
                &seq_name,
                &seq_name,
                kind.remark(),
                Local::now().naive_local(),
            )
            .hand_log(|msg| error!("{msg}: seq_name={seq_name}"))?;
            validate_sequence(&seq_name).await?;
        }
        Ok(())
    }

    pub async fn allocate(domain_id: &str, kind: SsrcKind) -> GlobalResult<String> {
        let seq_name = prefix(domain_id, kind)?;

        #[cfg(test)]
        if crate::storage::entity::test_storage_enabled() {
            let sequence = match kind {
                SsrcKind::Realtime => &TEST_REALTIME_SEQUENCE,
                SsrcKind::History => &TEST_HISTORY_SEQUENCE,
            };
            return Ok(format!("{seq_name}{:04}", next_test_value(sequence)));
        }

        for _ in 0..SSRC_SEQUENCE_MAX {
            let value = take_next_value(&seq_name).await?;
            let ssrc = format!("{seq_name}{value:04}");
            let numeric_ssrc = ssrc
                .parse::<u32>()
                .map_err(|_| invalid_sequence(&seq_name, "formatted SSRC is invalid"))?;
            if !crate::state::session::Cache::ssrc_is_active(numeric_ssrc)
                && !is_active(domain_id, &ssrc).await?
            {
                return Ok(ssrc);
            }
        }

        Err(GlobalError::new_biz_error(
            BaseErrorCode::IoBusy.code(),
            "SSRC sequence is exhausted",
            |msg| error!("{msg}: seq_name={seq_name}"),
        ))
    }
}

pub fn prefix(domain_id: &str, kind: SsrcKind) -> GlobalResult<String> {
    if domain_id.len() != 20 || !domain_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "Invalid session domain_id",
            |msg| error!("{msg}: expected 20 decimal digits, value={domain_id}"),
        ));
    }
    Ok(format!("{}{}", kind.marker(), &domain_id[3..8]))
}

async fn validate_sequence(seq_name: &str) -> GlobalResult<()> {
    let row = fetch_sequence_row(seq_name, false).await?;
    let Some((init_value, current_value, increment_value, prefix_code, code_length)) = row else {
        return Err(invalid_sequence(seq_name, "row is missing"));
    };
    validate_sequence_row(
        seq_name,
        init_value,
        current_value,
        increment_value,
        prefix_code.as_deref(),
        code_length,
    )
}

async fn fetch_sequence_row(
    seq_name: &str,
    lock: bool,
) -> GlobalResult<Option<(i64, i64, i32, Option<String>, Option<i32>)>> {
    match db::backend() {
        db::SessionDatabaseBackend::Mysql => {
            let sql = if lock {
                "SELECT init_value,current_value,increment_value,prefix_code,code_lenth FROM GB28181_SEQ_CODE WHERE seq_name=? FOR UPDATE"
            } else {
                "SELECT init_value,current_value,increment_value,prefix_code,code_lenth FROM GB28181_SEQ_CODE WHERE seq_name=?"
            };
            let row: Option<(u64, u64, i32, Option<String>, Option<i32>)> =
                db::fetch_optional_as!((u64, u64, i32, Option<String>, Option<i32>), sql, seq_name,)
                    .hand_log(|msg| error!("{msg}: seq_name={seq_name}"))?;
            row.map(|(init_value, current_value, increment_value, prefix_code, code_length)| {
                Ok((
                    i64::try_from(init_value)
                        .map_err(|_| invalid_sequence(seq_name, "init_value exceeds i64"))?,
                    i64::try_from(current_value)
                        .map_err(|_| invalid_sequence(seq_name, "current_value exceeds i64"))?,
                    increment_value,
                    prefix_code,
                    code_length,
                ))
            })
            .transpose()
        }
        db::SessionDatabaseBackend::Sqlite => db::fetch_optional_as!(
            (i64, i64, i32, Option<String>, Option<i32>),
            "SELECT init_value,current_value,increment_value,prefix_code,code_lenth FROM GB28181_SEQ_CODE WHERE seq_name=?",
            seq_name,
        )
        .hand_log(|msg| error!("{msg}: seq_name={seq_name}")),
    }
}

fn validate_sequence_row(
    seq_name: &str,
    init_value: i64,
    current_value: i64,
    increment_value: i32,
    prefix_code: Option<&str>,
    code_length: Option<i32>,
) -> GlobalResult<()> {
    if init_value != 1
        || !(1..=i64::from(SSRC_SEQUENCE_MAX)).contains(&current_value)
        || increment_value != 1
        || prefix_code != Some(seq_name)
        || code_length != Some(SSRC_CODE_LENGTH)
    {
        return Err(invalid_sequence(seq_name, "metadata is incompatible"));
    }
    Ok(())
}

async fn take_next_value(seq_name: &str) -> GlobalResult<u16> {
    let row = fetch_sequence_row(seq_name, true).await?;
    let current = current_sequence_value(seq_name, row.as_ref())?;
    let next = next_sequence_value(seq_name, row.as_ref())?;
    db::execute!(
        "UPDATE GB28181_SEQ_CODE SET current_value=? WHERE seq_name=?",
        next,
        seq_name
    )
    .hand_log(|msg| error!("{msg}: seq_name={seq_name}"))?;
    Ok(current)
}

fn next_sequence_value(
    seq_name: &str,
    row: Option<&(i64, i64, i32, Option<String>, Option<i32>)>,
) -> GlobalResult<i64> {
    let Some((init_value, current_value, increment_value, prefix_code, code_length)) = row else {
        return Err(invalid_sequence(seq_name, "row is missing"));
    };
    validate_sequence_row(
        seq_name,
        *init_value,
        *current_value,
        *increment_value,
        prefix_code.as_deref(),
        *code_length,
    )?;
    Ok(if *current_value == i64::from(SSRC_SEQUENCE_MAX) {
        *init_value
    } else {
        *current_value + 1
    })
}

fn current_sequence_value(
    seq_name: &str,
    row: Option<&(i64, i64, i32, Option<String>, Option<i32>)>,
) -> GlobalResult<u16> {
    let Some((_, current_value, _, _, _)) = row else {
        return Err(invalid_sequence(seq_name, "row is missing"));
    };
    u16::try_from(*current_value).map_err(|_| invalid_sequence(seq_name, "value exceeds u16"))
}

async fn is_active(signal_node_id: &str, ssrc: &str) -> GlobalResult<bool> {
    let row: Option<(i32,)> = db::fetch_optional_as!(
        (i32,),
        "SELECT 1 FROM GB28181_SIP_DIALOG_SESSION WHERE SIGNAL_NODE_ID=? AND SSRC=? AND STATE IN ('INVITING','ESTABLISHED','TERMINATING') AND EXPIRE_AT>? LIMIT 1",
        signal_node_id,
        ssrc,
        Local::now().naive_local(),
    )
    .hand_log(|msg| error!("{msg}: signal_node_id={signal_node_id}, ssrc={ssrc}"))?;
    Ok(row.is_some())
}

fn invalid_sequence(seq_name: &str, reason: &str) -> GlobalError {
    GlobalError::new_biz_error(
        BaseErrorCode::InvalidState.code(),
        "Invalid SSRC sequence configuration",
        |msg| error!("{msg}: seq_name={seq_name}, reason={reason}"),
    )
}

#[cfg(test)]
fn next_test_value(sequence: &AtomicU16) -> u16 {
    sequence
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            Some(if current >= SSRC_SEQUENCE_MAX {
                1
            } else {
                current + 1
            })
        })
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::{SsrcKind, next_test_value, prefix};
    use std::sync::atomic::AtomicU16;

    #[test]
    fn prefix_uses_protocol_positions_four_through_eight() {
        assert_eq!(
            prefix("34020000002000000001", SsrcKind::Realtime).unwrap(),
            "020000"
        );
        assert_eq!(
            prefix("34020000002000000001", SsrcKind::History).unwrap(),
            "120000"
        );
    }

    #[test]
    fn prefix_rejects_invalid_domain_id() {
        assert!(prefix("340200", SsrcKind::Realtime).is_err());
        assert!(prefix("3402000000200000000x", SsrcKind::Realtime).is_err());
    }

    #[test]
    fn test_sequence_wraps_without_zero() {
        let sequence = AtomicU16::new(9_999);
        assert_eq!(next_test_value(&sequence), 9_999);
        assert_eq!(next_test_value(&sequence), 1);
    }
}
