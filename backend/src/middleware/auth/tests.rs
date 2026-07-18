use super::*;
use axum::http::{header, HeaderValue};
use chrono::Utc;
use std::{net::IpAddr, path::PathBuf, sync::Arc};

fn test_config() -> AppConfig {
    AppConfig {
        database_url: "postgres://app:secret@localhost/app".into(),
        jwt_secret: "test jwt secret".into(),
        database_encryption_key: "test database encryption key".into(),
        previous_database_encryption_key: None,
        backup_dir: PathBuf::from("/tmp/dumply-tests"),
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        admin_username: "admin".into(),
        admin_password: "password".into(),
        reset_admin_password_on_start: false,
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
        token_version: 0,
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
    assert_eq!(auth.token_version, user.token_version);

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
async fn login_throttle_delays_after_repeated_failures_and_clears_on_success() {
    let state = AuthState::new();
    let peer_addr = SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 12345);

    let keys = state
        .check_login_allowed(peer_addr, " Admin ")
        .await
        .unwrap();
    for _ in 0..MAX_LOGIN_FAILURES {
        state.record_login_failure(&keys).await;
    }

    let delayed_at = std::time::Instant::now();
    assert!(state.check_login_allowed(peer_addr, "Admin").await.is_ok());
    assert!(delayed_at.elapsed() >= LOGIN_DELAY_STEP);

    state.clear_login_failures(&keys).await;
    assert!(state.check_login_allowed(peer_addr, "Admin").await.is_ok());
}

#[tokio::test]
async fn login_throttle_is_per_account_even_when_requests_share_a_proxy_ip() {
    let state = AuthState::new();
    let proxy_addr = SocketAddr::new(IpAddr::from([172, 18, 0, 2]), 12345);
    let admin_keys = state
        .check_login_allowed(proxy_addr, "admin")
        .await
        .unwrap();

    for _ in 0..MAX_LOGIN_FAILURES {
        state.record_login_failure(&admin_keys).await;
    }

    let delayed_at = std::time::Instant::now();
    assert!(state.check_login_allowed(proxy_addr, "admin").await.is_ok());
    assert!(delayed_at.elapsed() >= LOGIN_DELAY_STEP);
    assert!(state
        .check_login_allowed(proxy_addr, "another-admin")
        .await
        .is_ok());
}

#[test]
fn require_admin_rejects_non_admin_users() {
    assert!(require_admin(&AuthUser {
        id: Uuid::new_v4(),
        username: "admin".into(),
        role: "admin".into(),
        token_version: 0,
    })
    .is_ok());
    assert!(matches!(
        require_admin(&AuthUser {
            id: Uuid::new_v4(),
            username: "operator".into(),
            role: "operator".into(),
            token_version: 0,
        }),
        Err(AppError::Forbidden)
    ));
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to a disposable PostgreSQL database"]
async fn active_authorization_rejects_a_deleted_user_or_stale_token_version() {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to a disposable PostgreSQL database");
    let pool = crate::db::connect(&database_url).await.unwrap();
    crate::db::migrate(&pool).await.unwrap();
    let user = User {
        id: Uuid::new_v4(),
        username: format!("deleted-token-test-{}", Uuid::new_v4()),
        password_hash: "not-used-by-this-test".into(),
        role: "admin".into(),
        token_version: 0,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    sqlx::query("INSERT INTO users (id, username, password_hash, role) VALUES ($1, $2, $3, $4)")
        .bind(user.id)
        .bind(&user.username)
        .bind(&user.password_hash)
        .bind(&user.role)
        .execute(&pool)
        .await
        .unwrap();

    let config = test_config();
    let (token, _) = create_token(&user, &config).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    let state = AuthState::new();
    assert!(state
        .authorize_active(&headers, &config, &pool)
        .await
        .is_ok());

    sqlx::query("UPDATE users SET token_version = token_version + 1 WHERE id = $1")
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        state.authorize_active(&headers, &config, &pool).await,
        Err(AppError::Unauthorized)
    ));

    sqlx::query("UPDATE users SET token_version = 0 WHERE id = $1")
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        state.authorize_active(&headers, &config, &pool).await,
        Err(AppError::Unauthorized)
    ));
}
