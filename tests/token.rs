use std::time::{SystemTime, UNIX_EPOCH};

use cf_integration::token::{TokenKind, make_token, make_token_at};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde_json::{Value, json};
use uuid::Uuid;

const SECRET: &str = "my-test-key-but-now-longer-than-32-bytes";
const SUBJECT: &str = "admin@example.com";
const NOW: u64 = 1_700_000_000;

fn fixed_jti() -> Uuid {
    Uuid::parse_str("00000000-0000-0000-0000-000000000001").expect("fixed test UUID must be valid")
}

fn decode_verified(token: &str, secret: &str) -> Value {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = false;
    validation.set_audience(&["mcpgateway-api"]);
    validation.set_issuer(&["mcpgateway"]);

    decode::<Value>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .expect("token must verify")
    .claims
}

#[test]
fn scoped_token_has_hs256_signature_and_exact_claims() {
    let token = make_token_at(
        SECRET,
        SUBJECT,
        TokenKind::Scoped {
            server_id: Some("server-123".to_owned()),
        },
        NOW,
        fixed_jti(),
    )
    .expect("token generation must succeed");

    let header = decode_header(&token).expect("token header must decode");
    assert_eq!(header.alg, Algorithm::HS256);
    assert_eq!(header.typ.as_deref(), Some("JWT"));
    assert_eq!(
        decode_verified(&token, SECRET),
        json!({
            "username": SUBJECT,
            "sub": SUBJECT,
            "jti": "00000000-0000-0000-0000-000000000001",
            "token_use": "api",
            "iss": "mcpgateway",
            "aud": "mcpgateway-api",
            "iat": NOW,
            "nbf": NOW,
            "exp": NOW + 86_400,
            "teams": null,
            "user": {
                "email": SUBJECT,
                "full_name": "CLI User",
                "is_admin": true,
                "auth_provider": "cli"
            },
            "scopes": {
                "server_id": "server-123",
                "permissions": [
                    "servers.read",
                    "servers.use",
                    "tools.read",
                    "tools.call"
                ],
                "ip_restrictions": [],
                "time_restrictions": null
            }
        })
    );
}

#[test]
fn scoped_token_preserves_a_null_server_id() {
    let token = make_token_at(
        SECRET,
        SUBJECT,
        TokenKind::Scoped { server_id: None },
        NOW,
        fixed_jti(),
    )
    .expect("token generation must succeed");

    assert_eq!(
        decode_verified(&token, SECRET)["scopes"]["server_id"],
        Value::Null
    );
}

#[test]
fn admin_token_has_session_use_and_omits_scopes() {
    let token = make_token_at(SECRET, SUBJECT, TokenKind::Admin, NOW, fixed_jti())
        .expect("token generation must succeed");

    assert_eq!(
        decode_verified(&token, SECRET),
        json!({
            "username": SUBJECT,
            "sub": SUBJECT,
            "jti": "00000000-0000-0000-0000-000000000001",
            "token_use": "session",
            "iss": "mcpgateway",
            "aud": "mcpgateway-api",
            "iat": NOW,
            "nbf": NOW,
            "exp": NOW + 86_400,
            "teams": null,
            "user": {
                "email": SUBJECT,
                "full_name": "CLI User",
                "is_admin": true,
                "auth_provider": "cli"
            }
        })
    );
}

#[test]
fn verification_rejects_a_different_secret() {
    let token = make_token_at(SECRET, SUBJECT, TokenKind::Admin, NOW, fixed_jti())
        .expect("token generation must succeed");
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = false;
    validation.set_audience(&["mcpgateway-api"]);
    validation.set_issuer(&["mcpgateway"]);

    let result = decode::<Value>(
        &token,
        &DecodingKey::from_secret(b"different-secret"),
        &validation,
    );

    assert!(result.is_err());
}

#[test]
fn token_generation_rejects_expiration_overflow_without_exposing_inputs() {
    let secret = "do-not-expose-this-secret";
    let subject = "do-not-expose-this-subject";

    let error = make_token_at(secret, subject, TokenKind::Admin, u64::MAX, fixed_jti())
        .expect_err("expiration overflow must fail");
    let message = format!("{error:#}");

    assert!(message.contains("expiration"));
    assert!(!message.contains(secret));
    assert!(!message.contains(subject));
}

#[test]
fn production_token_uses_current_time_and_a_v4_uuid() {
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test clock must be after the Unix epoch")
        .as_secs();
    let token = make_token(SECRET, SUBJECT, TokenKind::Admin)
        .expect("production token generation must succeed");
    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test clock must be after the Unix epoch")
        .as_secs();
    let claims = decode_verified(&token, SECRET);

    let issued_at = claims["iat"].as_u64().expect("iat must be an integer");
    assert!((before..=after).contains(&issued_at));
    assert_eq!(claims["nbf"], issued_at);
    assert_eq!(claims["exp"], issued_at + 86_400);
    let jti = Uuid::parse_str(claims["jti"].as_str().expect("jti must be a string"))
        .expect("jti must be a UUID");
    assert_eq!(jti.get_version_num(), 4);
}
