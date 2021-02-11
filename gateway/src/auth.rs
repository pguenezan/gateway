use std::collections::HashSet;
use std::env;

use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use jsonwebtoken::errors::Error;
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
    pub token_id: String,
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

    static ref VALIDATION_2: Validation = Validation {
        leeway: 0,
        validate_exp: true,
        algorithms: vec![Algorithm::RS256],
        validate_nbf: false,
        iss: env::var("JWT_ISSER_2").ok(),
        aud: get_audience(),
        sub: None,
    };

    static ref PUBLIC_KEY_SHORT: DecodingKey<'static> =
        DecodingKey::from_rsa_pem(include_bytes!("public_key_short.pem")).unwrap();
    static ref PUBLIC_KEY_LONG: DecodingKey<'static> =
        DecodingKey::from_rsa_pem(include_bytes!("public_key_long.pem")).unwrap();

    static ref PUBLIC_KEY_SHORT_2: Result<DecodingKey<'static>, Error> =
        DecodingKey::from_rsa_pem(include_bytes!("public_key_short_2.pem"));
    static ref PUBLIC_KEY_LONG_2: Result<DecodingKey<'static>, Error> =
        DecodingKey::from_rsa_pem(include_bytes!("public_key_long_2.pem"));
}

const AUTH_SHIFT: usize = "Bearer ".len();

pub async fn get_claims(authorization: &str) -> Option<(Claims, &'static str)> {
    if authorization.len() <= AUTH_SHIFT {
        return None;
    }
    println!("{:#?}", &PUBLIC_KEY_LONG_2.is_ok());
    println!("{:#?}", &PUBLIC_KEY_SHORT_2.is_ok());
    match decode::<Claims>(&authorization[AUTH_SHIFT..], &PUBLIC_KEY_SHORT, &VALIDATION) {
        Ok(token) => return Some((token.claims, "short")),
        Err(_) => (),
    }
    match decode::<Claims>(&authorization[AUTH_SHIFT..], &PUBLIC_KEY_LONG, &VALIDATION) {
        Ok(token) => return Some((token.claims, "long")),
        Err(_) => (),
    }
    match PUBLIC_KEY_SHORT_2.as_ref() {
        Err(_) => (),
        Ok(key) => match decode::<Claims>(&authorization[AUTH_SHIFT..], key, &VALIDATION_2) {
            Ok(token) => return Some((token.claims, "short")),
            Err(_) => (),
        }
    }
    match PUBLIC_KEY_LONG_2.as_ref() {
        Err(_) => (),
        Ok(key) => match decode::<Claims>(&authorization[AUTH_SHIFT..], key, &VALIDATION_2) {
            Ok(token) => return Some((token.claims, "long")),
            Err(_) => return None,
        },
    }
    None
}
