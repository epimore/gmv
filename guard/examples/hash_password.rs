use std::io::{self, Read};

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, SaltString};
use uuid::Uuid;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut password = String::new();
    io::stdin().read_to_string(&mut password)?;
    let password = password.trim_end_matches(['\r', '\n']);
    if password.is_empty() {
        return Err("password must not be empty".into());
    }
    let salt = SaltString::encode_b64(Uuid::new_v4().as_bytes())
        .map_err(|error| io::Error::other(error.to_string()))?;
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|error| io::Error::other(error.to_string()))?;
    println!("{hash}");
    Ok(())
}
