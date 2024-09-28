use common::log::{error};
use common::err::{GlobalResult, TransError};
use common::yaml_rust::Yaml;

#[derive(Debug)]
pub struct Http {
    pub port: u16,
    pub timeout: u32,
    pub prefix: String,
    pub server_name: String,
    pub version: String,
}

impl Http {
    pub fn build(cfg: &Yaml) -> Self {
        if cfg.is_badvalue() || cfg["http"].is_badvalue() {
            Http {
                port: 8080,
                timeout: 30000,
                prefix: "gmv".to_string(),
                server_name: "web-server".to_string(),
                version: "v0.1".to_string(),
            }
        } else {
            let http = &cfg["http"];
            Http {
                port: http["port"].as_i64().unwrap_or(8080) as u16,
                timeout: http["timeout"].as_i64().unwrap_or(30000) as u32,
                prefix: http["prefix"].as_str().unwrap_or("gmv").to_string(),
                server_name: http["server_name"].as_str().unwrap_or("web-server").to_string(),
                version: http["version"].as_str().unwrap_or("v0.1").to_string(),
            }
        }
    }

    pub async fn init_web_server<T: 'static + poem_openapi::OpenApi>(&self, api: T) -> GlobalResult<()> {
        use poem::{Server, Route, EndpointExt};
        use poem::listener::TcpListener;
        use poem::middleware::Cors;
        use poem::http::Method;
        use poem_openapi::OpenApiService;

        let http_addr = format!("http://0.0.0.0:{}{}", &self.port, &self.prefix);
        let service = OpenApiService::new(api, &self.server_name, &self.version)
            .server(&http_addr);
        let ui = service.swagger_ui();
        let route = Route::new()
            .nest(&self.prefix, service
                .with(Cors::new().allow_methods([Method::GET, Method::POST])))
            .nest("/docs", ui);
        println!("Listen to http web addr = 0.0.0.0:{}\n ... GMV:SESSION started.\r\n", &self.port);
        Server::new(TcpListener::bind(format!("0.0.0.0:{}", &self.port)))
            .run(route).await.hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }
}