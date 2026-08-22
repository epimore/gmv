use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult};

pub const DEFAULT_PAGE_SIZE: usize = 100;
pub const MAX_PAGE_SIZE: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorQuery {
    pub after_id: Option<String>,
    pub limit: usize,
}

impl Default for CursorQuery {
    fn default() -> Self {
        Self {
            after_id: None,
            limit: DEFAULT_PAGE_SIZE,
        }
    }
}

impl CursorQuery {
    pub fn validate(&self) -> GlobalResult<()> {
        if self.limit == 0 || self.limit > MAX_PAGE_SIZE {
            return Err(GlobalError::new_biz_error(
                BaseErrorCode::InvalidRequest.code(),
                &format!("cursor limit must be 1..={MAX_PAGE_SIZE}"),
                |msg| base::log::error!("{msg}"),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorPage<T> {
    pub items: Vec<T>,
    pub next_after_id: Option<String>,
}
