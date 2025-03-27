use poem::web::Multipart;
use poem_openapi::{OpenApi, param::Query};
use common::log::error;
use crate::service;
use crate::utils::se_token;

pub struct SeApi;

#[OpenApi(prefix_path = "/es")]
impl SeApi {
    #[allow(non_snake_case)]
    #[oai(path = "/pic/upload", method = "post")]
    async fn pic_upload(&self,
                        #[oai(name = "token")] token: Query<String>,
                        #[oai(name = "sessionId")] sessionId: Query<String>,
                        #[oai(name = "fileId")] fileId: Query<Option<String>>,
                        #[oai(name = "snapShotFileID")] snapShotFileID: Query<Option<String>>,
                        mut multipart: Multipart) {
        if se_token::check_token(sessionId.0.as_str(),token.0.as_str()).is_ok() {
            loop {
                match multipart.next_field().await {
                    Ok(Some(field)) => {
                        let _ = service::biz::upload(field, sessionId.0.clone(), fileId.0.clone(), snapShotFileID.0.clone()).await;
                    }
                    Ok(None) => { break; }
                    Err(err) => {
                        error!("{}",err)
                    }
                }
            }
        }
    }
}