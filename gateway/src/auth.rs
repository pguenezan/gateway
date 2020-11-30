use std::collections::HashSet;
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
    pub roles: Vec<String>,
}

fn get_audience() -> Option<HashSet<String>> {
    match env::var("JWT_AUDIENCE") {
        Ok(aud) => {
            let mut auds = HashSet::new();
            auds.insert(aud);
            Some(auds)
        }
        Err(_) => None,
    }
}

lazy_static! {
    static ref VALIDATION: Validation = Validation {
        leeway: 0,
        validate_exp: true,
        algorithms: vec![Algorithm::RS256],
        validate_nbf: false,
        iss: env::var("JWT_ISSER").ok(),
        aud: get_audience(),
        sub: None,
    };
    static ref PUBLIC_KEY_SHORT: DecodingKey<'static> =
        DecodingKey::from_rsa_pem(include_bytes!("public_key_short.pem")).unwrap();
    static ref PUBLIC_KEY_LONG: DecodingKey<'static> =
        DecodingKey::from_rsa_pem(include_bytes!("public_key_long.pem")).unwrap();
}

const AUTH_SHIFT: usize = "Bearer ".len();

pub async fn get_claims(authorization: &str) -> Option<(Claims, &'static str)> {
    if authorization.len() <= AUTH_SHIFT {
        return None;
    }
    match decode::<Claims>(&authorization[AUTH_SHIFT..], &PUBLIC_KEY_SHORT, &VALIDATION) {
        Ok(token) => Some((token.claims, "short")),
        Err(_) => {
            let token =
                decode::<Claims>(&authorization[AUTH_SHIFT..], &PUBLIC_KEY_LONG, &VALIDATION)
                    .ok()?;
            Some((token.claims, "long"))
        }
    }
}
