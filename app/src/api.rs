//! The three Anthropic endpoints Claude Code itself uses for a subscription login (read from its
//! 2.1.x source): the usage meter, the profile (who a token belongs to) and the token refresh.
//! None is a documented public API; they are called the way Claude Code calls them, and rarely.
//! Tokens are never logged.

use std::time::Duration;

use serde_json::{Value, json};

const BASE_API_URL: &str = "https://api.anthropic.com";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
/// Claude Code's OAuth client id (its production config).
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const OAUTH_BETA: &str = "oauth-2025-04-20";

#[derive(Debug)]
pub enum ApiError {
    /// 401: the access token expired or is not accepted.
    Unauthorized,
    /// 403 "OAuth token has been revoked".
    Revoked,
    /// 400 invalid_grant on refresh: the refresh token is dead (used elsewhere, revoked, expired).
    InvalidGrant,
    /// 429, with Retry-After seconds when given.
    RateLimited(Option<u64>),
    Http(u16, String),
    Network(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized => write!(f, "the access token was not accepted (401)"),
            Self::Revoked => write!(f, "the login was revoked"),
            Self::InvalidGrant => write!(f, "the saved login is no longer valid"),
            Self::RateLimited(_) => write!(f, "rate limited by the usage endpoint"),
            Self::Http(code, body) => write!(f, "HTTP {code}: {body}"),
            Self::Network(e) => write!(f, "network: {e}"),
        }
    }
}

pub struct Api {
    http: reqwest::Client,
    user_agent: String,
}

impl Api {
    pub fn new(claude_version: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .expect("an HTTP client with default settings builds");
        Self {
            http,
            user_agent: format!("claude-cli/{claude_version} (external, cli)"),
        }
    }

    /// `GET /api/oauth/usage`: `five_hour`, `seven_day`, `limits[]`, `extra_usage`, ...
    pub async fn usage(&self, access_token: &str) -> Result<Value, ApiError> {
        let req = self
            .http
            .get(format!("{BASE_API_URL}/api/oauth/usage"))
            .bearer_auth(access_token)
            .header("anthropic-beta", OAUTH_BETA)
            .header("Content-Type", "application/json")
            .header("User-Agent", &self.user_agent);
        read(req.send().await).await
    }

    /// `GET /api/oauth/profile`: `{account: {uuid, email, ...}, organization: {uuid, ...}}`.
    pub async fn profile(&self, access_token: &str) -> Result<Value, ApiError> {
        let req = self
            .http
            .get(format!("{BASE_API_URL}/api/oauth/profile"))
            .bearer_auth(access_token)
            .header("Content-Type", "application/json")
            .header("Cache-Control", "no-cache")
            .header("User-Agent", &self.user_agent);
        read(req.send().await).await
    }

    /// The refresh grant. Answers `access_token`, `refresh_token` (rotated), `expires_in`,
    /// `refresh_token_expires_in`, `scope`.
    pub async fn refresh(&self, refresh_token: &str, scopes: &[String]) -> Result<Value, ApiError> {
        let mut body = json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": CLIENT_ID,
        });
        if !scopes.is_empty() {
            body["scope"] = Value::String(scopes.join(" "));
        }
        let req = self
            .http
            .post(TOKEN_URL)
            .header("Content-Type", "application/json")
            .header("User-Agent", &self.user_agent)
            .json(&body);
        read(req.send().await).await
    }
}

async fn read(sent: reqwest::Result<reqwest::Response>) -> Result<Value, ApiError> {
    let resp = sent.map_err(|e| ApiError::Network(e.without_url().to_string()))?;
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    let text = resp
        .text()
        .await
        .map_err(|e| ApiError::Network(e.without_url().to_string()))?;
    match status {
        200 => serde_json::from_str(&text).map_err(|e| ApiError::Http(200, format!("unreadable answer: {e}"))),
        401 => Err(ApiError::Unauthorized),
        403 if text.contains("revoked") => Err(ApiError::Revoked),
        429 => Err(ApiError::RateLimited(retry_after)),
        400 if text.contains("invalid_grant") => Err(ApiError::InvalidGrant),
        _ => Err(ApiError::Http(status, short(&text))),
    }
}

/// An error body, cut to one readable line.
fn short(text: &str) -> String {
    let message = serde_json::from_str::<Value>(text).ok().and_then(|v| {
        v.pointer("/error/message")
            .or_else(|| v.get("error_description"))
            .or_else(|| v.get("message"))
            .or_else(|| v.get("error"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    let line = message.unwrap_or_else(|| text.split_whitespace().collect::<Vec<_>>().join(" "));
    line.chars().take(160).collect()
}

/// The stored `claudeAiOauth` after a refresh answer `resp` at `now_ms` (Claude Code's merge: the
/// fields a refresh answers replace, the rest stay).
pub fn merge_refreshed(old: &Value, resp: &Value, now_ms: i64) -> Option<Value> {
    let access = resp.get("access_token")?.as_str()?;
    let expires_in = resp.get("expires_in")?.as_f64()?;
    let mut creds = old.clone();
    let obj = creds.as_object_mut()?;
    obj.insert("accessToken".into(), access.into());
    if let Some(rt) = resp.get("refresh_token").and_then(Value::as_str) {
        obj.insert("refreshToken".into(), rt.into());
    }
    obj.insert("expiresAt".into(), (now_ms + (expires_in * 1000.0) as i64).into());
    if let Some(rte) = resp.get("refresh_token_expires_in").and_then(Value::as_f64) {
        obj.insert("refreshTokenExpiresAt".into(), (now_ms + (rte * 1000.0) as i64).into());
    }
    if let Some(scope) = resp.get("scope").and_then(Value::as_str) {
        let scopes: Vec<Value> = scope.split_whitespace().map(|s| Value::String(s.to_owned())).collect();
        obj.insert("scopes".into(), Value::Array(scopes));
    }
    Some(creds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refresh_replaces_tokens_and_keeps_plan_fields() {
        let old = json!({"accessToken": "a1", "refreshToken": "r1", "expiresAt": 1, "scopes": ["x"],
                         "subscriptionType": "team", "rateLimitTier": "default_claude_max_5x"});
        let resp = json!({"access_token": "a2", "refresh_token": "r2", "expires_in": 28800,
                          "refresh_token_expires_in": 2592000, "scope": "user:inference user:profile"});
        let new = merge_refreshed(&old, &resp, 1_000).unwrap();
        assert_eq!(new["accessToken"], "a2");
        assert_eq!(new["refreshToken"], "r2");
        assert_eq!(new["expiresAt"], 1_000 + 28_800_000);
        assert_eq!(new["refreshTokenExpiresAt"], 1_000 + 2_592_000_000_i64);
        assert_eq!(new["scopes"], json!(["user:inference", "user:profile"]));
        assert_eq!(new["subscriptionType"], "team");
    }

    #[test]
    fn error_bodies_become_one_line() {
        assert_eq!(short(r#"{"error":{"type":"x","message":"Nope"}}"#), "Nope");
        assert_eq!(short("plain\n text"), "plain text");
    }
}
