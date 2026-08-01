use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::Response;
use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::err::{BaseErrorCode, CodeOutErr};
use base::exception::{BizError, GlobalError, GlobalResult, GlobalResultExt};
use base::log::{debug, error};
use base::serde::{Deserialize, Serialize};
use base::serde_default;
use base::tokio_util::sync::CancellationToken;
use gmv_domain::info::res::Resp;
use std::net::SocketAddr;
use std::path::PathBuf;

pub(crate) mod cloud_recording;
mod edge;
pub(crate) mod image;

#[derive(Debug, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "http", check)]
pub struct Http {
    #[serde(default = "default_listen_addr")]
    pub listen_addr: SocketAddr,
    #[serde(default = "default_public_url")]
    pub public_url: String,
    #[serde(default)]
    pub tls: HttpTlsConf,
}

#[derive(Debug, Default, Deserialize)]
#[serde(crate = "base::serde")]
pub struct HttpTlsConf {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub certificate_path: PathBuf,
    #[serde(default)]
    pub private_key_path: PathBuf,
}
serde_default!(
    default_listen_addr,
    SocketAddr,
    "0.0.0.0:8080".parse().expect("valid default HTTP address")
);
serde_default!(
    default_public_url,
    String,
    "http://127.0.0.1:8080".to_string()
);

impl CheckFromConf for Http {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        if self.listen_addr.port() == 0 {
            return Err(FieldCheckError::BizError(
                "http.listen_addr端口不能为0".to_string(),
            ));
        }
        self.public_endpoint()
            .map(|_| ())
            .map_err(FieldCheckError::BizError)?;
        if self.tls.enabled
            && (self.tls.certificate_path.as_os_str().is_empty()
                || self.tls.private_key_path.as_os_str().is_empty())
        {
            return Err(FieldCheckError::BizError(
                "http.tls启用时certificate_path和private_key_path不能为空".to_string(),
            ));
        }
        Ok(())
    }
}

impl Http {
    pub fn get_http_by_conf() -> Self {
        Http::conf()
    }

    pub fn listen_http_server(&self) -> GlobalResult<std::net::TcpListener> {
        let listener =
            std::net::TcpListener::bind(self.listen_addr).hand_log(|msg| error!("{msg}"))?;
        Ok(listener)
    }

    pub fn public_endpoint(&self) -> Result<(bool, String, u16), String> {
        let url = url::Url::parse(&self.public_url)
            .map_err(|error| format!("http.public_url地址无效: {error}"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err("http.public_url必须使用http或https".to_string());
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err("http.public_url不能包含凭据".to_string());
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err("http.public_url不能包含query或fragment".to_string());
        }
        let host = url
            .host_str()
            .filter(|host| !host.trim().is_empty())
            .ok_or_else(|| "http.public_url必须包含host".to_string())?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| "http.public_url必须包含有效端口".to_string())?;
        Ok((url.scheme() == "https", host.to_string(), port))
    }

    pub async fn run(
        &self,
        listener: std::net::TcpListener,
        cancel_token: CancellationToken,
    ) -> GlobalResult<()> {
        listener
            .set_nonblocking(true)
            .hand_log(|msg| error!("{msg}"))?;
        let app = routes();
        let service = app.into_make_service_with_connect_info::<SocketAddr>();
        let handle = axum_server::Handle::new();
        let shutdown = handle.clone();
        let shutdown_cancel = cancel_token.clone();
        base::tokio::spawn(async move {
            shutdown_cancel.cancelled().await;
            debug!("HTTP graceful shutdown requested by cancellation token");
            shutdown.graceful_shutdown(None);
        });
        let result = if self.tls.enabled {
            let rustls = axum_server::tls_rustls::RustlsConfig::from_pem_file(
                self.tls.certificate_path.clone(),
                self.tls.private_key_path.clone(),
            )
            .await
            .hand_log(|msg| error!("{msg}"))?;
            axum_server::from_tcp_rustls(listener, rustls)
                .hand_log(|msg| error!("{msg}"))?
                .handle(handle)
                .serve(service)
                .await
        } else {
            axum_server::from_tcp(listener)
                .hand_log(|msg| error!("{msg}"))?
                .handle(handle)
                .serve(service)
                .await
        };
        match result.hand_log(|msg| error!("{msg}")) {
            Ok(()) => {
                debug!(
                    "HTTP server exited; cancellation_requested={}",
                    cancel_token.is_cancelled()
                );
                Ok(())
            }
            error => error,
        }
    }

    /// 404 Not Found
    pub fn res_404() -> Response<Body> {
        Response::builder()
            .header("Content-Type", "text/plain")
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("404 Not Found"))
            .unwrap()
    }

    /// 401 Unauthorized
    pub fn res_401() -> Response<Body> {
        Response::builder()
            .header("Content-Type", "text/plain")
            .status(StatusCode::UNAUTHORIZED)
            .body(Body::from("401 Unauthorized"))
            .unwrap()
    }

    /// 500 Internal Server Error
    pub fn res_500() -> Response<Body> {
        Response::builder()
            .header("Content-Type", "text/plain")
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from("500 Internal Server Error"))
            .unwrap()
    }
}

pub(crate) fn routes() -> Router {
    Router::new()
        .nest("/edge", edge::routes())
        .merge(cloud_recording::routes())
        .merge(image::routes())
}

pub fn res_by_error<T: Serialize>(err: GlobalError) -> Resp<T> {
    let code = match &err {
        GlobalError::BizErr(BizError { code, .. }) => *code,
        GlobalError::SysErr(_) => BaseErrorCode::Internal.code(),
    };
    Resp::build_failed_code(code, err.out_err().into_owned())
}

pub fn get_gmv_token(headers: HeaderMap) -> GlobalResult<String> {
    let header_name = HeaderName::from_static("gmv-token");
    if let Some(value) = headers.get(&header_name) {
        match value.to_str() {
            Ok(token) => Ok(token.to_string()),
            Err(_) => Err(GlobalError::new_biz_error(
                BaseErrorCode::Unauthorized.code(),
                "Gmv-Token is invalid",
                |msg| error!("{}", msg),
            )),
        }
    } else {
        Err(GlobalError::new_biz_error(
            BaseErrorCode::Unauthorized.code(),
            "Gmv-Token not found",
            |msg| error!("{}", msg),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::Http;
    use base::cfg_lib::conf::CheckFromConf;

    #[test]
    fn public_https_is_independent_from_plain_local_listener() {
        let conf = Http {
            listen_addr: "0.0.0.0:28567".parse().unwrap(),
            public_url: "https://gmv.example.com/session-1".to_string(),
            tls: Default::default(),
        };

        assert_eq!(
            conf.public_endpoint().unwrap(),
            (true, "gmv.example.com".to_string(), 443)
        );
        conf._field_check().unwrap();
    }

    #[test]
    fn public_url_rejects_credentials_query_and_unsupported_scheme() {
        for public_url in [
            "ftp://gmv.example.com/session-1",
            "https://user:pass@gmv.example.com/session-1",
            "https://gmv.example.com/session-1?node=1",
        ] {
            let conf = Http {
                listen_addr: "0.0.0.0:28567".parse().unwrap(),
                public_url: public_url.to_string(),
                tls: Default::default(),
            };
            assert!(conf.public_endpoint().is_err());
        }
    }

    #[test]
    fn http_config_ignores_extra_fields() {
        let yaml = r#"
listen_addr: 0.0.0.0:28567
public_url: https://session.example.com
enabled: false
port: 18080
"#;
        let conf: Http = base::serde_yaml::from_str(yaml).unwrap();
        assert_eq!(conf.listen_addr, "0.0.0.0:28567".parse().unwrap());
        assert_eq!(conf.public_url, "https://session.example.com");
    }
}
