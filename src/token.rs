//! Native JWT generation for control-plane and dataplane requests.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use uuid::Uuid;

const ISSUER: &str = "mcpgateway";
const AUDIENCE: &str = "mcpgateway-api";
const TTL_SECONDS: u64 = 86_400;
const PERMISSIONS: &[&str] = &["servers.read", "servers.use", "tools.read", "tools.call"];

/// Selects the claims emitted for a generated token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenKind {
    /// API token restricted to an optional virtual server.
    Scoped { server_id: Option<String> },
    /// Administrative control-plane session token.
    Admin,
}

#[derive(Serialize)]
struct Claims<'a> {
    username: &'a str,
    sub: &'a str,
    jti: String,
    token_use: &'static str,
    iss: &'static str,
    aud: &'static str,
    iat: u64,
    nbf: u64,
    exp: u64,
    teams: Option<()>,
    user: UserClaims<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scopes: Option<Scopes>,
}

#[derive(Serialize)]
struct UserClaims<'a> {
    email: &'a str,
    full_name: &'static str,
    is_admin: bool,
    auth_provider: &'static str,
}

#[derive(Serialize)]
struct Scopes {
    server_id: Option<String>,
    permissions: &'static [&'static str],
    ip_restrictions: &'static [&'static str],
    time_restrictions: Option<()>,
}

/// Generates a token using the current system clock and a random v4 UUID.
///
/// # Errors
///
/// Returns an error when the system clock predates the Unix epoch, the
/// expiration timestamp overflows, or JWT encoding fails.
pub fn make_token(secret: &str, subject: &str, kind: TokenKind) -> Result<String> {
    let now_epoch_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("failed to generate token because the system clock predates the Unix epoch")?
        .as_secs();

    make_token_at(secret, subject, kind, now_epoch_seconds, Uuid::new_v4())
}

/// Generates a deterministic token from explicit time and UUID inputs.
///
/// # Errors
///
/// Returns an error when the expiration timestamp overflows or JWT encoding
/// fails.
pub fn make_token_at(
    secret: &str,
    subject: &str,
    kind: TokenKind,
    now_epoch_seconds: u64,
    jti: Uuid,
) -> Result<String> {
    let expiration = now_epoch_seconds.checked_add(TTL_SECONDS).ok_or_else(|| {
        anyhow!("failed to calculate token expiration because the timestamp overflows")
    })?;
    let (token_use, scopes) = match kind {
        TokenKind::Scoped { server_id } => (
            "api",
            Some(Scopes {
                server_id,
                permissions: PERMISSIONS,
                ip_restrictions: &[],
                time_restrictions: None,
            }),
        ),
        TokenKind::Admin => ("session", None),
    };
    let claims = Claims {
        username: subject,
        sub: subject,
        jti: jti.to_string(),
        token_use,
        iss: ISSUER,
        aud: AUDIENCE,
        iat: now_epoch_seconds,
        nbf: now_epoch_seconds,
        exp: expiration,
        teams: None,
        user: UserClaims {
            email: subject,
            full_name: "CLI User",
            is_admin: true,
            auth_provider: "cli",
        },
        scopes,
    };
    let mut header = Header::new(Algorithm::HS256);
    header.typ = Some("JWT".to_owned());

    encode(
        &header,
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .context("failed to encode JWT")
}
