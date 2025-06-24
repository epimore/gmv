use common::log::warn;
use poem_openapi::{OpenApi, param::Query};
use crate::service;
use crate::utils::se_token;
use poem::web::Multipart;

pub struct SeApi;

#[OpenApi(prefix_path = "/es")]
impl SeApi {
    #[allow(non_snake_case)]
    #[oai(path = "/pic/upload", method = "post", ignore_case)]
    async fn pic_upload(&self,
                        #[oai(name = "token")] token: Query<String>,
                        #[oai(name = "SessionID")] SessionID: Query<String>,
                        #[oai(name = "FileID")] FileID: Query<Option<String>>,
                        #[oai(name = "SnapShotFileID")] SnapShotFileID: Query<Option<String>>,
                        mut multipart: Multipart,) {
        if se_token::check_token(SessionID.0.as_str(), token.0.as_str()).is_ok() {
            match multipart.next_field().await {
                Ok(Some(field)) => {let _ = service::biz::upload(field, SessionID.0.clone(), FileID.0.clone(), SnapShotFileID.0.clone()).await;}
                Ok(None) => {warn!("no field");}
                Err(err) => {
                    warn!("{:?}", err);
                }
            }
        }
    }
}