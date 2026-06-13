mod config;
mod db;
mod error;
mod middleware;
mod routes;
mod services;

use std::{sync::Arc, time::Duration};

use axum::{
    http::{header, HeaderValue, Method, StatusCode},
    Router,
};
use config::AppConfig;
use middleware::auth::AuthState;
use services::{backup_executor::BackupRuntime, scheduler::BackupScheduler};
use sqlx::PgPool;
use tokio::sync::Semaphore;
use tower_http::{
    cors::CorsLayer, limit::RequestBodyLimitLayer, timeout::TimeoutLayer, trace::TraceLayer,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub config: Arc<AppConfig>,
    pub backup_runtime: BackupRuntime,
    pub scheduler: Arc<BackupScheduler>,
    pub auth: Arc<AuthState>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    let config = Arc::new(AppConfig::from_env()?);
    let db = db::connect(&config.database_url).await?;
    db::migrate(&db).await?;
    db::bootstrap_admin(&db, &config).await?;

    let backup_runtime = BackupRuntime {
        db: db.clone(),
        config: config.clone(),
        permits: Arc::new(Semaphore::new(config.max_concurrent_backups)),
    };
    let scheduler = Arc::new(BackupScheduler::new(backup_runtime.clone()).await?);
    scheduler.reload_all().await?;
    scheduler.start().await?;

    let state = AppState {
        db,
        config,
        backup_runtime,
        scheduler,
        auth: Arc::new(AuthState::new()),
    };
    let bind_addr = state.config.bind_addr;
    let cors = cors_layer(&state.config)?;
    let app = Router::new()
        .nest("/api", routes::router(state.clone()))
        .layer(RequestBodyLimitLayer::new(1024 * 1024))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(3600),
        ))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(addr = %bind_addr, "server listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}

fn cors_layer(config: &AppConfig) -> anyhow::Result<CorsLayer> {
    let Some(origin) = &config.cors_allowed_origin else {
        return Ok(CorsLayer::new());
    };

    if origin == "*" {
        return Ok(CorsLayer::permissive());
    }

    Ok(CorsLayer::new()
        .allow_origin(origin.parse::<HeaderValue>()?)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(error = ?err, "failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => tracing::error!(error = ?err, "failed to install SIGTERM handler"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received");
}
