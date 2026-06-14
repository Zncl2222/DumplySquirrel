use super::*;
use axum::http::{header, HeaderValue};
use chrono::Utc;
use std::{net::IpAddr, path::PathBuf, sync::Arc};

fn test_config() -> AppConfig {
    AppConfig {
        database_url: "postgres://app:secret@localhost/app".into(),
        jwt_secret: "test jwt secret".into(),
        database_encryption_key: "test database encryption key".into(),
        backup_dir: PathBuf::from("/tmp/dumply-tests"),
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        admin_username: "admin".into(),
        admin_password: "password".into(),
        jwt_ttl_seconds: 3600,
        max_concurrent_backups: 2,
        cors_allowed_origin: None,
        smtp: None,
    }
}

fn test_user(role: &str) -> User {
    User {
        id: Uuid::new_v4(),
        username: "admin".into(),
        password_hash: "hash".into(),
        role: role.into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

fn auth_headers(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    headers
}

#[test]
fn authorize_accepts_valid_token_and_rejects_revoked_token() {
    let state = AuthState::new();
    let config = Arc::new(test_config());
    let user = test_user("admin");
    let (token, _) = create_token(&user, &config).unwrap();
    let headers = auth_headers(&token);

    let auth = state.authorize(&headers, &config).unwrap();
    assert_eq!(auth.id, user.id);
    assert_eq!(auth.username, user.username);
    assert_eq!(auth.role, user.role);

    state.revoke_token(&headers, &config).unwrap();
    assert!(matches!(
        state.authorize(&headers, &config),
        Err(AppError::Unauthorized)
    ));
}

#[test]
fn authorize_rejects_missing_bearer_token() {
    let state = AuthState::new();
    let config = test_config();

    assert!(matches!(
        state.authorize(&HeaderMap::new(), &config),
        Err(AppError::Unauthorized)
    ));
}

#[tokio::test]
async fn login_throttle_blocks_after_repeated_failures_and_clears_on_success() {
    let state = AuthState::new();
    let peer_addr = SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 12345);

    let keys = state
        .check_login_allowed(peer_addr, " Admin ")
        .await
        .unwrap();
    for _ in 0..MAX_LOGIN_FAILURES {
        state.record_login_failure(&keys).await;
    }

    assert!(matches!(
        state.check_login_allowed(peer_addr, "admin").await,
        Err(AppError::TooManyRequests)
    ));

    state.clear_login_failures(&keys).await;
    assert!(state.check_login_allowed(peer_addr, "admin").await.is_ok());
}

#[test]
fn require_admin_rejects_non_admin_users() {
    assert!(require_admin(&AuthUser {
        id: Uuid::new_v4(),
        username: "admin".into(),
        role: "admin".into(),
    })
    .is_ok());
    assert!(matches!(
        require_admin(&AuthUser {
            id: Uuid::new_v4(),
            username: "operator".into(),
            role: "operator".into(),
        }),
        Err(AppError::Forbidden)
    ));
}
