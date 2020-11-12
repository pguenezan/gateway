use hyper::header::{HeaderMap, HeaderValue};
use std::env;

use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

use lazy_static::lazy_static;

#[allow(dead_code)] // some fields are only used by the validator
#[derive(Deserialize)]
pub struct Claims {
    pub sub: String,
    iss: String,
    exp: usize,
    pub preferred_username: String,
    pub given_name: String,
    pub family_name: String,
    pub email: String,
}

lazy_static! {
    static ref VALIDATION: Validation = Validation {
        leeway: 0,
        validate_exp: true,
        algorithms: vec![Algorithm::RS256],
        validate_nbf: false,
        iss: env::var("JWT_ISSER").ok(),
        aud: None,
        sub: None,
    };
    static ref PUBLIC_KEY: DecodingKey<'static> =
        DecodingKey::from_rsa_pem(include_bytes!("public_key.pem")).unwrap();
}

const AUTH_HEADER_NAME: &str = "authorization";
const AUTH_SHIFT: usize = "Bearer ".len();

pub async fn get_claims(headers: &HeaderMap<HeaderValue>) -> Option<Claims> {
    let authorization = headers.get(AUTH_HEADER_NAME)?.to_str().ok()?;
    let token = decode::<Claims>(&authorization[AUTH_SHIFT..], &PUBLIC_KEY, &VALIDATION).ok()?;
    return Some(token.claims);
}
