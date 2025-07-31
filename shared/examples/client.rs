use common::{serde, tokio};
use feign::re_exports::{reqwest, serde_json};
use feign::{client, ClientResult, HttpMethod, RequestBody};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use common::serde::{Deserialize, Serialize};
use common::tokio::time;

#[derive(Serialize,Deserialize,Debug)]
#[serde(crate = "common::serde")]
struct CreateUser {
    name: String,
}

// the output to our `create_user` handler
#[derive(Serialize,Deserialize,Debug)]
#[serde(crate = "common::serde")]
struct User {
    id: u64,
    name: String,
}
async fn client_builder() -> ClientResult<reqwest::Client> {
    Ok(reqwest::ClientBuilder::new().build()?)
}

async fn before_send(
    request_builder: reqwest::RequestBuilder,
    http_method: HttpMethod,
    host: String,
    client_path: String,
    request_path: String,
    body: RequestBody,
    headers: Option<HashMap<String, String>>,
) -> ClientResult<reqwest::RequestBuilder> {
    println!(
        "============= (Before_send)\n\
            {:?} => {}{}{}\n\
            {:?}\n\
            {:?}",
        http_method, host, client_path, request_path, headers, body
    );
    Ok(request_builder.header("a", "b"))
}

// async fn bare_string(body: String) -> ClientResult<String> {
//     Ok(body)
// }
//
async fn decode<T: for<'de> serde::Deserialize<'de>>(body: String) -> ClientResult<T> {
    Ok(serde_json::from_str(body.as_str())?)
}

// #[client(
//     host = "http://127.0.0.1:3030",
//     path = "",
//     client_builder = "client_builder",
//     before_send = "before_send"
// )]
#[client(
    path = "",
    client_builder = "client_builder"
)]
pub trait UserClient {
    #[get(path = "", deserialize = "decode")]
    async fn hello(&self) -> ClientResult<Option<CreateUser>>;
    #[post(path = "/new_user")]
    async fn new_user(&self, #[json] user: &CreateUser) -> ClientResult<Option<User>>;
    // #[post(path = "/new_user", deserialize = "bare_string")]
    // async fn new_user_bare_string(&self, #[json] user: &User) -> ClientResult<String>;
    // #[get(path = "/headers")]
    // async fn headers(
    //     &self,
    //     #[json] age: &i64,
    //     #[headers] headers: HashMap<String, String>,
    // ) -> ClientResult<Option<User>>;
}

#[tokio::main]
async fn main() {
    let user_client: UserClient = UserClient::builder()
        .set_host_arc(Arc::new(String::from("http://127.0.0.1:30000")))
        .build();

    match user_client.hello().await {
        Ok(option) => match option {
            Some(msg) => println!("user : {:?}", msg),
            None => println!("none"),
        },
        Err(err) => eprintln!("{}", err),
    };

    match user_client
        .new_user(&CreateUser {
            name: "aaaa".to_owned()
        })
        .await
    {
        Ok(option) => match option {
            Some(result) => println!("result : {:?}", result),
            None => println!("none"),
        },
        Err(err) => eprintln!("{}", err),
    };

    // match user_client
    //     .new_user_bare_string(&User {
    //         id: 123,
    //         name: "name".to_owned(),
    //     })
    //     .await
    // {
    //     Ok(result) => println!("result : {}", result),
    //     Err(err) => eprintln!("{}", err),
    // };
    // 
    // let mut headers = HashMap::<String, String>::new();
    // headers.insert(String::from("C"), String::from("D"));
    // 
    // match user_client.headers(&12, headers).await {
    //     Ok(option) => match option {
    //         Some(user) => println!("user : {}", user.name),
    //         None => println!("none"),
    //     },
    //     Err(err) => eprintln!("{}", err),
    // };
    time::sleep(Duration::from_secs(10)).await;
}