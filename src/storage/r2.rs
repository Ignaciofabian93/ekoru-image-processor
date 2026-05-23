use aws_sdk_s3::primitives::ByteStream;
use uuid::Uuid;

use crate::{error::AppError, state::AppState};

pub async fn upload(
    state: &AppState,
    bytes: &[u8],
    entity: &str,
    entity_id: &str,
) -> Result<String, AppError> {
    let key = format!("{entity}/{entity_id}/{}.webp", Uuid::new_v4());

    state
        .s3
        .put_object()
        .bucket(&state.cfg.r2_bucket)
        .key(&key)
        .body(ByteStream::from(bytes.to_vec()))
        .content_type("image/webp")
        .cache_control("public, max-age=31536000, immutable")
        .send()
        .await
        .map_err(|e| AppError::Storage(format!("put_object: {e}")))?;

    Ok(key)
}

pub async fn delete(state: &AppState, key: &str) -> Result<(), AppError> {
    state
        .s3
        .delete_object()
        .bucket(&state.cfg.r2_bucket)
        .key(key)
        .send()
        .await
        .map_err(|e| AppError::Storage(format!("delete_object: {e}")))?;

    Ok(())
}
