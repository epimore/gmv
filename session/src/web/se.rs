use poem::web::Multipart;
use poem_openapi::{OpenApi,param::Query};
use common::log::error;
use crate::service;

pub struct SeApi;

#[OpenApi(prefix_path = "/es")]
impl SeApi {
    #[allow(non_snake_case)]
    #[oai(path = "/pic/upload", method = "post")]
    async fn pic_upload(&self,
                        #[oai(name = "uk")] uk: Query<String>,
                        #[oai(name = "sessionId")] sessionId: Query<Option<String>>,
                        #[oai(name = "fileId")] fileId: Query<Option<String>>,
                        #[oai(name = "snapShotFileID")] snapShotFileID: Query<Option<String>>,
                        mut multipart: Multipart) {
        loop {
            match multipart.next_field().await {
                Ok(Some(field)) => {
                    let _ = service::control::upload(field, uk.0.clone(), sessionId.0.clone(), fileId.0.clone(), snapShotFileID.0.clone()).await;
                }
                Ok(None) => { break; }
                Err(err) => {
                    error!("{}",err)
                }
            }
        }
    }
}