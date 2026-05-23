use axum::{
    extract::{Path, State},
    http::StatusCode,
};

use crate::{error::AppError, state::AppState, storage};

pub async fn handle_delete(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<StatusCode, AppError> {
    if key.is_empty() || key.contains("..") {
        return Err(AppError::InvalidField("key".into()));
    }

    storage::r2::delete(&state, &key).await?;
    Ok(StatusCode::NO_CONTENT)
}
