use std::collections::HashSet;
use std::process::exit;

use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

use once_cell::sync::OnceCell;

use crate::runtime_config::{AuthSource, RUNTIME_CONFIG};

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

fn get_audience(aud: String) -> HashSet<String> {
    let mut auds = HashSet::new();
    auds.insert(aud);
    auds
}

struct TokenSource {
    pub name: String,
    pub token_type: String,
    pub validation: Validation,
    pub public_key: DecodingKey<'static>,
}

impl TokenSource {
    pub fn new(auth_source: &'static AuthSource) -> Self {
        let validation = Validation {
            leeway: 0,
            validate_exp: true,
            algorithms: vec![Algorithm::RS256],
            validate_nbf: false,
            iss: Some(auth_source.issuer.to_string()),
            aud: Some(get_audience(auth_source.audience.to_string())),
            sub: None,
        };
        let public_key = DecodingKey::from_rsa_pem(auth_source.public_key.as_bytes()).unwrap();
        Self {
            name: auth_source.name.to_string(),
            token_type: auth_source.token_type.to_string(),
            validation,
            public_key,
        }
    }
}

static TOKEN_SOURCES: OnceCell<Vec<TokenSource>> = OnceCell::new();

const AUTH_SHIFT: usize = "Bearer ".len();

pub fn init_token_sources() {
    let token_sources = RUNTIME_CONFIG
        .get()
        .unwrap()
        .auth_sources
        .iter()
        .map(|auth_source| TokenSource::new(auth_source))
        .collect();
    if TOKEN_SOURCES.set(token_sources).is_err() {
        eprintln!("fail to set TOKEN_SOURCES");
        exit(1);
    }
}

pub async fn get_claims(authorization: &str) -> Option<(Claims, String)> {
    if authorization.len() <= AUTH_SHIFT {
        return None;
    }
    for token_source in TOKEN_SOURCES.get().unwrap().iter().as_ref() {
        match decode::<Claims>(
            &authorization[AUTH_SHIFT..],
            &token_source.public_key,
            &token_source.validation,
        ) {
            Ok(token) => return Some((token.claims, token_source.token_type.to_string())),
            Err(e) => {
                println!("{} {}", token_source.name, e);
            }
        }
    }
    None
}
