use crate::storage::Store;
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::Html,
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
type Reply = Result<Json<Value>, (StatusCode, Json<Value>)>;
fn failure(e: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error":{"code":"STORE_ERROR","message":e.to_string()}})),
    )
}
async fn query(db: PathBuf, which: &'static str) -> Reply {
    tokio::task::spawn_blocking(move || {
        let store = Store::read_only(&db)?;
        match which {
            "sources" => store.sources(),
            "status" => store.status(),
            table => store.projections(table),
        }
    })
    .await
    .map_err(failure)?
    .map(Json)
    .map_err(failure)
}
#[derive(Deserialize)]
struct Page {
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    cursor: i64,
}
fn default_limit() -> usize {
    100
}
async fn events(
    State(db): State<PathBuf>,
    page: Result<Query<Page>, axum::extract::rejection::QueryRejection>,
) -> Reply {
    let Query(p) = page.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"INVALID_QUERY","message":e.to_string()}})),
        )
    })?;
    if !(1..=500).contains(&p.limit) || p.cursor < 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(
                json!({"error":{"code":"INVALID_QUERY","message":"limit must be 1..500; cursor must be nonnegative"}}),
            ),
        ));
    }
    tokio::task::spawn_blocking(move || Store::read_only(&db)?.events(p.limit, p.cursor))
        .await
        .map_err(failure)?
        .map(Json)
        .map_err(failure)
}
pub fn router(db: PathBuf) -> Router {
    Router::new()
        .route("/",get(|| async { Html(include_str!("../web/index.html")) }))
        .route("/healthz",get(|| async { Json(json!({"service":"gimps-watch","status":"ok","scope":"HTTP process only; inspect sources for observation health"})) }))
        .route("/v1/status",get(|State(db): State<PathBuf>| query(db,"status")))
        .route("/v1/sources",get(|State(db): State<PathBuf>| query(db,"sources")))
        .route("/v1/workers",get(|State(db): State<PathBuf>| query(db,"workers")))
        .route("/v1/assignments",get(|State(db): State<PathBuf>| query(db,"assignments")))
        .route("/v1/events",get(events)).with_state(db)
}
