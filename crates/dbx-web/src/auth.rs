use std::sync::Arc;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::extract::State;
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::state::{AnonymousSshSession, WebState};

#[derive(Deserialize)]
pub struct LoginRequest {
    pub password: String,
}

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

#[derive(Serialize)]
pub struct AuthCheckResponse {
    pub authenticated: bool,
    pub required: bool,
    pub setup_required: bool,
}

const MAX_ATTEMPTS: u32 = 5;
const LOCKOUT_SECS: u64 = 60;
const ANONYMOUS_SSH_SESSION_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
const MAX_ANONYMOUS_SSH_SESSIONS: usize = 1024;

fn session_cookie_path(state: &WebState) -> &str {
    state.public_base_path.as_str()
}

fn api_path_suffix<'a>(path: &'a str, public_base_path: &str) -> Option<&'a str> {
    if let Some(suffix) = path.strip_prefix("/api/") {
        return Some(suffix);
    }
    let base = public_base_path.trim_end_matches('/');
    if base.is_empty() || base == "/" {
        return None;
    }
    path.strip_prefix(base)?.strip_prefix("/api/")
}

fn middleware_api_path_suffix<'a>(path: &'a str, public_base_path: &str) -> Option<&'a str> {
    if let Some(suffix) = api_path_suffix(path, public_base_path) {
        return Some(suffix);
    }

    let base = public_base_path.trim_end_matches('/');
    if !base.is_empty() && base != "/" && path.strip_prefix(base).is_some() {
        return None;
    }

    path.strip_prefix('/').filter(|suffix| !suffix.is_empty())
}

pub async fn login(State(state): State<Arc<WebState>>, Json(body): Json<LoginRequest>) -> Result<Response, StatusCode> {
    let hash_guard = state.password_hash.read().await;
    let hash_str = match hash_guard.as_deref() {
        Some(h) => h.to_string(),
        None => {
            return Ok((StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response());
        }
    };
    drop(hash_guard);

    // Check rate limit
    {
        let rl = state.login_rate_limit.lock().await;
        if let Some(locked_until) = rl.locked_until {
            if locked_until > std::time::Instant::now() {
                let remaining = (locked_until - std::time::Instant::now()).as_secs();
                return Ok((
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(serde_json::json!({"error": format!("Please try again in {remaining}s")})),
                )
                    .into_response());
            }
        }
    }

    let parsed_hash = PasswordHash::new(&hash_str).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if Argon2::default().verify_password(body.password.as_bytes(), &parsed_hash).is_err() {
        let mut rl = state.login_rate_limit.lock().await;
        rl.fail_count += 1;
        if rl.fail_count >= MAX_ATTEMPTS {
            rl.locked_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(LOCKOUT_SECS));
            rl.fail_count = 0;
        }
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Success — reset rate limit
    {
        let mut rl = state.login_rate_limit.lock().await;
        rl.fail_count = 0;
        rl.locked_until = None;
    }

    let token = uuid::Uuid::new_v4().to_string();
    state.sessions.write().await.insert(token.clone());

    let cookie = format!("dbx_session={token}; Path={}; HttpOnly; SameSite=Lax", session_cookie_path(&state));
    Ok((StatusCode::OK, [("set-cookie", cookie.as_str())], Json(serde_json::json!({"ok": true}))).into_response())
}

pub async fn setup(State(state): State<Arc<WebState>>, Json(body): Json<LoginRequest>) -> Result<Response, StatusCode> {
    if state.password_disabled {
        return Err(StatusCode::FORBIDDEN);
    }

    // Only allow setup when no password is configured
    if state.password_hash.read().await.is_some() {
        return Err(StatusCode::FORBIDDEN);
    }

    if body.password.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(body.password.as_bytes(), &salt)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .to_string();

    // Save to database
    state.app.storage.save_password_hash(&hash).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Update in-memory state
    *state.password_hash.write().await = Some(hash);

    // Auto-login: create session
    let token = uuid::Uuid::new_v4().to_string();
    state.sessions.write().await.insert(token.clone());

    let cookie = format!("dbx_session={token}; Path={}; HttpOnly; SameSite=Lax", session_cookie_path(&state));
    Ok((StatusCode::OK, [("set-cookie", cookie.as_str())], Json(serde_json::json!({"ok": true}))).into_response())
}

pub async fn check(State(state): State<Arc<WebState>>, req: Request<axum::body::Body>) -> Json<AuthCheckResponse> {
    if state.password_disabled {
        return Json(AuthCheckResponse { authenticated: true, required: false, setup_required: false });
    }
    let has_password = state.password_hash.read().await.is_some();
    if !has_password {
        return Json(AuthCheckResponse { authenticated: false, required: false, setup_required: true });
    }
    let authenticated = match extract_session_token(&req) {
        Some(token) => state.sessions.read().await.contains(&token),
        None => false,
    };
    Json(AuthCheckResponse { authenticated, required: true, setup_required: false })
}

pub async fn change_password(
    State(state): State<Arc<WebState>>,
    Json(body): Json<ChangePasswordRequest>,
) -> Result<Response, StatusCode> {
    let hash_guard = state.password_hash.read().await;
    let hash_str = match hash_guard.as_deref() {
        Some(h) => h.to_string(),
        None => return Err(StatusCode::BAD_REQUEST),
    };
    drop(hash_guard);

    if body.new_password.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let parsed_hash = PasswordHash::new(&hash_str).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if Argon2::default().verify_password(body.old_password.as_bytes(), &parsed_hash).is_err() {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let salt = SaltString::generate(&mut OsRng);
    let new_hash = Argon2::default()
        .hash_password(body.new_password.as_bytes(), &salt)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .to_string();

    state.app.storage.save_password_hash(&new_hash).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    *state.password_hash.write().await = Some(new_hash);

    Ok((StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response())
}

pub async fn logout(State(state): State<Arc<WebState>>, req: Request<axum::body::Body>) -> Response {
    if let Some(token) = extract_session_token(&req) {
        release_ssh_owners(&state, std::slice::from_ref(&token)).await;
        state.anonymous_ssh_sessions.write().await.remove(&token);
        state.sessions.write().await.remove(&token);
    }
    let cookie = format!("dbx_session=; Path={}; HttpOnly; Max-Age=0", session_cookie_path(&state));
    (StatusCode::OK, [("set-cookie", cookie.as_str())], Json(serde_json::json!({"ok": true}))).into_response()
}

pub fn session_token_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    let cookie_header = headers.get("cookie")?.to_str().ok()?;
    for pair in cookie_header.split(';') {
        let pair = pair.trim();
        if let Some(value) = pair.strip_prefix("dbx_session=") {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

pub async fn require_session_token(state: &WebState, headers: &axum::http::HeaderMap) -> Result<String, StatusCode> {
    let token = session_token_from_headers(headers).ok_or(StatusCode::UNAUTHORIZED)?;
    if state.sessions.read().await.contains(&token) {
        return Ok(token);
    }
    if state.password_disabled
        && state
            .anonymous_ssh_sessions
            .read()
            .await
            .get(&token)
            .is_some_and(|session| session.last_seen.elapsed() <= ANONYMOUS_SSH_SESSION_TTL)
    {
        return Ok(token);
    }
    Err(StatusCode::UNAUTHORIZED)
}

fn extract_session_token<B>(req: &Request<B>) -> Option<String> {
    session_token_from_headers(req.headers())
}

pub async fn auth_middleware(
    State(state): State<Arc<WebState>>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    // Auth endpoints are always accessible.
    let api_suffix = middleware_api_path_suffix(req.uri().path(), &state.public_base_path);
    if api_suffix.is_some_and(|suffix| suffix.starts_with("auth/")) {
        return next.run(req).await;
    }

    // Non-API requests (static files) are always accessible.
    if api_suffix.is_none() {
        return next.run(req).await;
    }

    if state.password_disabled {
        if !requires_anonymous_ssh_owner(api_suffix) {
            return next.run(req).await;
        }

        let requested_token = extract_session_token(&req);
        let (token, created, expired_owners) = issue_anonymous_ssh_session(&state, requested_token.as_deref()).await;
        release_ssh_owners(&state, &expired_owners).await;
        let cookie_pair = format!("dbx_session={token}");
        if let Ok(value) = HeaderValue::from_str(&cookie_pair) {
            req.headers_mut().insert(header::COOKIE, value);
        }
        let mut response = next.run(req).await;
        if created {
            let cookie = format!("dbx_session={token}; Path={}; HttpOnly; SameSite=Lax", session_cookie_path(&state));
            if let Ok(value) = HeaderValue::from_str(&cookie) {
                response.headers_mut().append(header::SET_COOKIE, value);
            }
        }
        return response;
    }

    if state.password_hash.read().await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    // Check session token
    if let Some(token) = extract_session_token(&req) {
        if state.sessions.read().await.contains(&token) {
            return next.run(req).await;
        }
    }

    StatusCode::UNAUTHORIZED.into_response()
}

async fn issue_anonymous_ssh_session(state: &WebState, requested_token: Option<&str>) -> (String, bool, Vec<String>) {
    let now = std::time::Instant::now();
    let mut sessions = state.anonymous_ssh_sessions.write().await;
    let mut removed = prune_anonymous_ssh_sessions(&mut sessions, now);
    if let Some(token) = requested_token {
        if let Some(session) = sessions.get_mut(token) {
            session.last_seen = now;
            return (token.to_string(), false, removed);
        }
    }

    let token = uuid::Uuid::new_v4().to_string();
    sessions.insert(token.clone(), AnonymousSshSession { last_seen: now });
    removed.extend(enforce_anonymous_ssh_session_limit(&mut sessions, &token));
    (token, true, removed)
}

pub async fn cleanup_expired_anonymous_ssh_sessions(state: &WebState) -> usize {
    let expired = {
        let mut sessions = state.anonymous_ssh_sessions.write().await;
        prune_anonymous_ssh_sessions(&mut sessions, std::time::Instant::now())
    };
    let count = expired.len();
    release_ssh_owners(state, &expired).await;
    count
}

fn requires_anonymous_ssh_owner(api_suffix: Option<&str>) -> bool {
    api_suffix.is_some_and(|suffix| suffix.starts_with("ssh/"))
}

fn prune_anonymous_ssh_sessions(
    sessions: &mut std::collections::HashMap<String, AnonymousSshSession>,
    now: std::time::Instant,
) -> Vec<String> {
    let expired: Vec<String> = sessions
        .iter()
        .filter(|(_, session)| now.saturating_duration_since(session.last_seen) > ANONYMOUS_SSH_SESSION_TTL)
        .map(|(token, _)| token.clone())
        .collect();
    for token in &expired {
        sessions.remove(token);
    }
    expired
}

fn enforce_anonymous_ssh_session_limit(
    sessions: &mut std::collections::HashMap<String, AnonymousSshSession>,
    protected_token: &str,
) -> Vec<String> {
    let mut removed = Vec::new();
    while sessions.len() > MAX_ANONYMOUS_SSH_SESSIONS {
        let oldest = sessions
            .iter()
            .filter(|(candidate, _)| candidate.as_str() != protected_token)
            .min_by_key(|(_, session)| session.last_seen)
            .map(|(candidate, _)| candidate.clone());
        let Some(oldest) = oldest else {
            break;
        };
        sessions.remove(&oldest);
        removed.push(oldest);
    }
    removed
}

async fn release_ssh_owners(state: &WebState, owners: &[String]) {
    if owners.is_empty() {
        return;
    }
    for owner in owners {
        state.app.ssh_registry.close_sessions_by_owner(owner).await;
    }
    state.ssh_downloads.write().await.retain(|_, download| !owners.contains(&download.owner_session));
}

#[cfg(test)]
mod tests {
    use super::{
        api_path_suffix, enforce_anonymous_ssh_session_limit, middleware_api_path_suffix, prune_anonymous_ssh_sessions,
        requires_anonymous_ssh_owner, session_token_from_headers, ANONYMOUS_SSH_SESSION_TTL,
        MAX_ANONYMOUS_SSH_SESSIONS,
    };
    use crate::state::AnonymousSshSession;
    use std::collections::HashMap;

    #[test]
    fn api_path_suffix_handles_root_api_paths() {
        assert_eq!(api_path_suffix("/api/auth/check", "/"), Some("auth/check"));
        assert_eq!(api_path_suffix("/api/query/execute", "/"), Some("query/execute"));
        assert_eq!(api_path_suffix("/dbx/api/auth/check", "/"), None);
    }

    #[test]
    fn passwordless_anonymous_owner_is_limited_to_ssh_routes() {
        assert!(requires_anonymous_ssh_owner(Some("ssh/session")));
        assert!(requires_anonymous_ssh_owner(Some("ssh/terminal/ws")));
        assert!(!requires_anonymous_ssh_owner(Some("connection/list")));
        assert!(!requires_anonymous_ssh_owner(None));
    }

    #[test]
    fn anonymous_ssh_sessions_expire_and_are_bounded() {
        let started_at = std::time::Instant::now();
        let now = started_at + ANONYMOUS_SSH_SESSION_TTL + std::time::Duration::from_secs(1);
        let mut sessions = HashMap::from([
            ("expired".to_string(), AnonymousSshSession { last_seen: started_at }),
            ("active".to_string(), AnonymousSshSession { last_seen: now }),
        ]);
        assert_eq!(prune_anonymous_ssh_sessions(&mut sessions, now), vec!["expired".to_string()]);
        assert!(sessions.contains_key("active"));

        for index in 0..=MAX_ANONYMOUS_SSH_SESSIONS {
            sessions.insert(
                format!("session-{index}"),
                AnonymousSshSession { last_seen: now + std::time::Duration::from_millis(index as u64) },
            );
        }
        sessions.insert("protected".to_string(), AnonymousSshSession { last_seen: now });
        let removed = enforce_anonymous_ssh_session_limit(&mut sessions, "protected");
        assert!(!removed.is_empty());
        assert_eq!(sessions.len(), MAX_ANONYMOUS_SSH_SESSIONS);
        assert!(sessions.contains_key("protected"));
    }

    #[test]
    fn api_path_suffix_handles_mounted_api_paths() {
        assert_eq!(api_path_suffix("/dbx/api/auth/check", "/dbx"), Some("auth/check"));
        assert_eq!(api_path_suffix("/tools/dbx/api/query/execute", "/tools/dbx"), Some("query/execute"));
        assert_eq!(api_path_suffix("/dbx/login", "/dbx"), None);
    }

    #[test]
    fn middleware_api_path_suffix_handles_nested_router_paths() {
        assert_eq!(middleware_api_path_suffix("/auth/check", "/"), Some("auth/check"));
        assert_eq!(middleware_api_path_suffix("/connection/list", "/"), Some("connection/list"));
        assert_eq!(middleware_api_path_suffix("/api/connection/list", "/"), Some("connection/list"));
        assert_eq!(middleware_api_path_suffix("/dbx/api/connection/list", "/dbx"), Some("connection/list"));
        assert_eq!(middleware_api_path_suffix("/dbx/login", "/dbx"), None);
    }

    #[test]
    fn session_cookie_parser_extracts_only_the_dbx_session_cookie() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("cookie", "theme=dark; dbx_session=owner-token; other=value".parse().unwrap());
        assert_eq!(session_token_from_headers(&headers).as_deref(), Some("owner-token"));

        headers.insert("cookie", "not_dbx_session=wrong; theme=dark".parse().unwrap());
        assert_eq!(session_token_from_headers(&headers), None);
    }
}
