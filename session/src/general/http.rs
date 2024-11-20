use poem::http::Method;
use poem::listener::{TcpAcceptor};
use poem::middleware::Cors;
use poem::{EndpointExt, Route, Server};
use poem_openapi::OpenApiService;
use common::serde::{Deserialize};
use common::cfg_lib;
use common::cfg_lib::conf;
use common::serde_yaml;
use common::log::{error, info};
use common::exception::{GlobalResult, TransError};
use common::serde_default;
use crate::{web};

#[derive(Debug, Deserialize)]
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
        let listener =std::net::TcpListener::bind(format!("0.0.0.0:{}", self.port)).hand_log(|msg| error!("{msg}"))?;
        info!("Listen to http web addr = 0.0.0.0:{} ...", self.port);
        Ok(listener)
    }

    pub async fn run(&self, listener: std::net::TcpListener) -> GlobalResult<()> {
        let service = OpenApiService::new((web::api::RestApi, web::hook::HookApi), &self.server_name, &self.version)
            .server(format!("http://0.0.0.0:{}{}", &self.port, &self.prefix));
        let ui = service.swagger_ui();
        let route = Route::new()
            .nest(&self.prefix, service
                .with(Cors::new().allow_methods([Method::GET, Method::POST])))
            .nest("/docs", ui);
        let acceptor = TcpAcceptor::from_std(listener).hand_log(|msg| error!("{msg}"))?;
        info!("Web server start running 1111111111");
        Server::new_with_acceptor(acceptor).run(route).await.hand_log(|msg| error!("{msg}"))?;
        error!("http server exception:exited");
        Ok(())
    }
    //
    // pub async fn process_http_server() -> GlobalResult<()> {
    //     let http: Http = Http::conf();
    //     http.init_web_server((web::api::RestApi, web::hook::HookApi)).await?;
    //     error!("http server exception:exited");
    //     Ok(())
    // }
    //
    // async fn init_web_server<T: 'static + poem_openapi::OpenApi>(&self, api: T) -> GlobalResult<()> {
    //     use poem::{Server, Route, EndpointExt};
    //     use poem::listener::TcpListener;
    //     use poem::middleware::Cors;
    //     use poem::http::Method;
    //     use poem_openapi::OpenApiService;
    //
    //     let http_addr = format!("http://0.0.0.0:{}{}", &self.port, &self.prefix);
    //     let service = OpenApiService::new(api, &self.server_name, &self.version)
    //         .server(&http_addr);
    //     let ui = service.swagger_ui();
    //     let route = Route::new()
    //         .nest(&self.prefix, service
    //             .with(Cors::new().allow_methods([Method::GET, Method::POST])))
    //         .nest("/docs", ui);
    //     let acceptor = TcpListener::bind(format!("0.0.0.0:{}", &self.port)).into_acceptor().await.hand_log(|msg| error!("web start failed: {msg}")).unwrap();
    //     info!("Listen to http web addr = 0.0.0.0:{} ...\r\n", &self.port);
    //     eprintln!("Listen to http web addr = 0.0.0.0:{} ...\r\n", &self.port);
    //     Server::new_with_acceptor(acceptor)
    //         .run(route).await.hand_log(|msg| error!("{msg}"))?;
    //     // Server::new(TcpListener::bind(format!("0.0.0.0:{}", &self.port)))
    //     //     .run(route).await.hand_log(|msg| error!("{msg}"))?;
    //     Ok(())
    // }
}