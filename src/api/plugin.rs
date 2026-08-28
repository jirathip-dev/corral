use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Json;
use serde::Deserialize;

use super::AppState;
use crate::plugin::{self, ActionResult, CardResult, PluginView};

fn config_dir() -> PathBuf {
    std::env::var("CORRAL_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
                .join(".config/corral")
        })
}

pub(crate) async fn view(
    State(_state): State<Arc<AppState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    match plugin::discover(&config_dir()) {
        Ok(Some(manifest)) => {
            let cards = plugin::scheduled_cards(&manifest).await;
            let view = PluginView {
                name: manifest.name,
                version: manifest.version,
                cards,
                actions: manifest.actions,
            };
            (StatusCode::OK, Json(serde_json::to_value(view).unwrap()))
        }
        Ok(None) => (
            StatusCode::OK,
            Json(serde_json::json!({"installed": false, "cards": [], "actions": []})),
        ),
        Err(error) => (
            StatusCode::OK,
            Json(
                serde_json::json!({"installed": true, "cards": [{"id":"plugin", "title":"Fleet Ops", "value":null, "error":error}], "actions": []}),
            ),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct ActionRequest {
    pub action_id: String,
    pub confirmed: bool,
}

pub(crate) async fn action(
    Json(request): Json<ActionRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if !request.confirmed {
        return (StatusCode::OK, Json(serde_json::json!({"cancelled": true})));
    }
    match plugin::run_action(&config_dir(), &request.action_id, true).await {
        Ok(result) => (StatusCode::OK, Json(serde_json::to_value(result).unwrap())),
        Err(error) => (StatusCode::OK, Json(serde_json::json!({"error": error}))),
    }
}

#[allow(dead_code)]
fn _types_are_wire_stable(_: CardResult, _: ActionResult) {}
