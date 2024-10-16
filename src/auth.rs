use std::collections::HashSet;
use std::sync::LazyLock;

use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

use crate::runtime_config::{AuthSource, RUNTIME_CONFIG};

#[allow(dead_code)] // some fields are only used by the validator
#[derive(Deserialize, Debug)]
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

fn get_aud_or_iss(aud_or_iss: String) -> HashSet<String> {
    let mut hs = HashSet::new();
    hs.insert(aud_or_iss);
    hs
}

struct TokenSource {
    pub name: String,
    pub token_type: String,
    pub validation: Validation,
    pub public_key: DecodingKey,
}

impl TokenSource {
    pub fn new(auth_source: &'static AuthSource) -> Self {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.leeway = 0;
        validation.leeway = 0;
        validation.validate_exp = true;
        validation.validate_nbf = false;
        validation.iss = Some(get_aud_or_iss(auth_source.issuer.to_string()));
        validation.aud = Some(get_aud_or_iss(auth_source.audience.to_string()));
        validation.sub = None;
        let public_key = DecodingKey::from_rsa_pem(auth_source.public_key.as_bytes()).unwrap();
        Self {
            name: auth_source.name.to_string(),
            token_type: auth_source.token_type.to_string(),
            validation,
            public_key,
        }
    }
}

static TOKEN_SOURCES: LazyLock<Vec<TokenSource>> = LazyLock::new(|| {
    RUNTIME_CONFIG
        .auth_sources
        .iter()
        .map(TokenSource::new)
        .collect()
});

const AUTH_SHIFT: usize = "Bearer ".len();

pub async fn get_claims(authorization: &str) -> Option<(Claims, String)> {
    if authorization.len() <= AUTH_SHIFT {
        warn!("event='An error occurs while getting claim, no claim'");
        return None;
    }
    let mut errors = Vec::new();
    for token_source in TOKEN_SOURCES.iter().as_ref() {
        match decode::<Claims>(
            &authorization[AUTH_SHIFT..],
            &token_source.public_key,
            &token_source.validation,
        ) {
            Ok(token) => return Some((token.claims, token_source.token_type.to_string())),
            Err(e) => {
                errors.push(format!("{}: {}", token_source.name, e));
            }
        }
    }
    warn!("event='An error occurs while getting claim: {:?}'", errors);
    None
}
