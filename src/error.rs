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
            // Over the decoder limits (IMG-1): the request is wrong, not the image.
            AppError::Image(image::ImageError::Limits(_)) => StatusCode::BAD_REQUEST,
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

        let message = match &self {
            AppError::Image(image::ImageError::Limits(_)) => format!(
                "image too large: at most {} px per side and {} MB decoded",
                crate::processor::MAX_SOURCE_SIDE,
                crate::processor::MAX_DECODE_ALLOC / (1024 * 1024)
            ),
            other => other.to_string(),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}
