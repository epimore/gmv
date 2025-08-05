use poem::http::Method;
use poem::listener::{TcpAcceptor};
use poem::middleware::Cors;
use poem::{EndpointExt, Route, Server};
use poem_openapi::OpenApiService;
use common::serde::{Deserialize};
use common::cfg_lib::conf;
use common::log::{error, info};
use common::exception::{GlobalResult, GlobalResultExt};
use common::serde_default;
use crate::{web};

#[derive(Debug, Deserialize)]
#[serde(crate = "common::serde")]
#[conf(prefix = "http")]
pub struct Http {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_timeout")]
    pub timeout: u16,
    #[serde(default = "default_prefix")]
    pub prefix: String,
    #[serde(default = "default_server_name")]
    pub server_name: String,
    #[serde(default = "default_version")]
    pub version: String,
}
serde_default!(default_port, u16, 8080);
serde_default!(default_timeout, u16, 30);
serde_default!(default_prefix, String, "/gmv".to_string());
serde_default!(default_server_name, String, "web-server".to_string());
serde_default!(default_version, String, "v0.1".to_string());
impl Http {
    pub fn get_http_by_conf() -> Self {
        Http::conf()
    }
    pub fn listen_http_server(&self) -> GlobalResult<std::net::TcpListener> {
        let listener = std::net::TcpListener::bind(format!("0.0.0.0:{}", self.port)).hand_log(|msg| error!("{msg}"))?;
        info!("Listen to http web addr = 0.0.0.0:{} ...", self.port);
        Ok(listener)
    }

    pub async fn run(&self, listener: std::net::TcpListener) -> GlobalResult<()> {
        let service = OpenApiService::new((web::api::RestApi, web::hook::HookApi, web::se::SeApi), &self.server_name, &self.version)
            .server(format!("http://0.0.0.0:{}{}", &self.port, &self.prefix));
        let ui = service.swagger_ui();
        let route = Route::new()
            .nest(&self.prefix, service
                .with(Cors::new().allow_methods([Method::GET, Method::POST])))
            .nest("/docs", ui);
        listener.set_nonblocking(true).hand_log(|msg| error!("{msg}"))?;
        let acceptor = TcpAcceptor::from_std(listener).hand_log(|msg| error!("{msg}"))?;
        Server::new_with_acceptor(acceptor).run(route).await.hand_log(|msg| error!("{msg}"))?;
        error!("http server exception:exited");
        Ok(())
    }
}