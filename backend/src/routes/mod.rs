pub mod auth;
pub mod backups;
pub mod dashboard;
pub mod events;
pub mod history;
pub mod users;

use axum::{routing::get, Json, Router};
use serde_json::json;

use crate::AppState;

pub fn router(_state: AppState) -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .nest("/auth", auth::router())
        .nest("/users", users::router())
        .nest("/backup-configs", backups::router())
        .nest("/backup-events", events::router())
        .nest("/backup-history", history::router())
        .nest("/dashboard", dashboard::router())
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "data": { "status": "ok" } }))
}
