mod config;
mod db;
mod error;
mod middleware;
mod routes;
mod services;

use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{
    http::{header, HeaderValue, Method, StatusCode},
    Router,
};
use config::AppConfig;
use middleware::auth::AuthState;
use services::{
    backup_executor::{self, BackupRuntime},
    scheduler::BackupScheduler,
};
use sqlx::PgPool;
use tokio::sync::{Mutex, Semaphore};
use tower_http::{
    cors::CorsLayer, limit::RequestBodyLimitLayer, set_header::SetResponseHeaderLayer,
    timeout::TimeoutLayer, trace::TraceLayer,
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

    let mut config = AppConfig::from_env()?;
    let db = db::connect(&config.database_url).await?;
    db::migrate(&db).await?;
    let instance_lock = db::acquire_instance_lock(&db).await?;
    let instance_loss = instance_lock.loss_receiver();
    if let Some(previous_key) = config.previous_database_encryption_key.take() {
        let rotated = services::crypto::rotate_database_encryption_key(
            &db,
            &previous_key,
            &config.database_encryption_key,
        )
        .await?;
        tracing::warn!(
            rotated_configs = rotated,
            "database encryption-key rotation completed; remove DATABASE_ENCRYPTION_KEY_PREVIOUS before the next start"
        );
    }
    services::crypto::validate_database_encryption_key(&db, &config.database_encryption_key)
        .await?;
    db::bootstrap_admin(&db, &config).await?;
    let config = Arc::new(config);

    let backup_runtime = BackupRuntime {
        db: db.clone(),
        config: config.clone(),
        permits: Arc::new(Semaphore::new(config.max_concurrent_backups)),
        cancellations: Arc::new(Mutex::new(HashMap::new())),
    };
    backup_executor::recover_interrupted_backups(&backup_runtime).await?;
    let file_deletion_worker = services::file_deletion::spawn_file_deletion_worker(
        backup_runtime.db.clone(),
        backup_runtime.config.backup_dir.clone(),
    );
    let retention_worker = backup_executor::spawn_retention_worker(backup_runtime.clone());
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
            // Backup downloads return their streaming body promptly, so regular API work never
            // needs an hour-long request future. Bound stalled database/client operations while
            // allowing the response body itself to continue streaming through Nginx.
            Duration::from_secs(60),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(addr = %bind_addr, "server listening");
    let (shutdown_reason_sender, shutdown_reason_receiver) = tokio::sync::oneshot::channel();
    let server_result = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let instance_lease_lost = shutdown_signal(instance_loss).await;
        let _ = shutdown_reason_sender.send(instance_lease_lost);
    })
    .await;
    let instance_lease_lost = shutdown_reason_receiver.await.unwrap_or(false);
    retention_worker.abort();
    let _ = retention_worker.await;
    file_deletion_worker.abort();
    let _ = file_deletion_worker.await;
    instance_lock.release().await;
    server_result?;
    if instance_lease_lost {
        anyhow::bail!(
            "database instance lease was lost; backend shut down to prevent concurrent writers"
        );
    }

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

async fn shutdown_signal(mut instance_loss: tokio::sync::watch::Receiver<bool>) -> bool {
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

    let lease_lost = async move {
        if *instance_loss.borrow() {
            return;
        }
        while instance_loss.changed().await.is_ok() {
            if *instance_loss.borrow() {
                return;
            }
        }
    };

    let instance_lease_lost = tokio::select! {
        _ = ctrl_c => false,
        _ = terminate => false,
        _ = lease_lost => true,
    };

    if instance_lease_lost {
        tracing::error!("database instance lease lost; starting fail-closed shutdown");
        let _fail_closed_watchdog = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            tracing::error!("fail-closed shutdown deadline exceeded; terminating process");
            std::process::exit(1);
        });
    } else {
        tracing::info!("shutdown signal received");
    }
    instance_lease_lost
}
