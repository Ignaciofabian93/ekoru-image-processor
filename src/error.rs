use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("missing field: {0}")]
    MissingField(String),

    #[error("invalid field: {0}")]
    InvalidField(String),

    #[error("payload too large")]
    PayloadTooLarge,

    #[error("unauthorized")]
    Unauthorized,

    #[error("multipart error: {0}")]
    Multipart(#[from] axum::extract::multipart::MultipartError),

    #[error("image processing error: {0}")]
    Image(#[from] image::ImageError),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::MissingField(_) | AppError::InvalidField(_) | AppError::Multipart(_) => {
                StatusCode::BAD_REQUEST
            }
            AppError::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::Image(_) => StatusCode::UNPROCESSABLE_ENTITY,
            AppError::Storage(_) | AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        if status.is_server_error() {
            tracing::error!(error = %self, "server error");
        } else {
            tracing::warn!(error = %self, "client error");
        }

        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}
