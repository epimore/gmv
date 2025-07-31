use axum::{
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use common::serde::{Deserialize, Serialize};
use common::tokio;
use shared::info::res::Resp;

#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
struct CreateUser {
    name: String,
}

// the output to our `create_user` handler
#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
struct User {
    id: u64,
    name: String,
}

#[tokio::main]
async fn main() {

    // build our application with a route
    let app = Router::new()
        // `GET /` goes to `root`
        .route("/", get(root))
        .route("/put/user", post(put_user))
        // `POST /users` goes to `create_user`
        .route("/new_user", post(new_user));

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:30000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn put_user(Json(user): Json<User>) -> Json<Resp<()>> {
    println!("user: {:?}", &user);
    Json(Resp::build_success())
}

// basic handler that responds with a static string
async fn root() -> Json<CreateUser> {
    let user = CreateUser { name: "world".to_string() };
    Json(user)
}

async fn new_user(
    // this argument tells axum to parse the request body
    // as JSON into a `CreateUser` type
    Json(payload): Json<CreateUser>,
) -> (StatusCode, Json<User>) {
    // insert your application logic here
    let user = User {
        id: 1337,
        name: payload.name,
    };
    println!("user: {:?}", &user);
    // this will be converted into a JSON response
    // with a status code of `201 Created`
    (StatusCode::CREATED, Json(user))
}