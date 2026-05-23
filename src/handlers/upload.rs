use axum::{
    extract::{Multipart, State},
    Json,
};
use serde::Serialize;

use crate::{error::AppError, processor, state::AppState, storage};

#[derive(Serialize)]
pub struct UploadResponse {
    pub key: String,
    pub url: String,
    pub original_size: usize,
    pub processed_size: usize,
    pub width: u32,
    pub height: u32,
}

pub async fn handle_upload(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, AppError> {
    let mut image_bytes: Option<Vec<u8>> = None;
    let mut entity: Option<String> = None;
    let mut entity_id: Option<String> = None;

    while let Some(field) = multipart.next_field().await? {
        match field.name().unwrap_or("") {
            "file" => {
                let bytes = field.bytes().await?;
                if bytes.len() > state.cfg.max_upload_bytes {
                    return Err(AppError::PayloadTooLarge);
                }
                image_bytes = Some(bytes.to_vec());
            }
            "entity" => entity = Some(field.text().await?),
            "entity_id" => entity_id = Some(field.text().await?),
            _ => {
                let _ = field.bytes().await?;
            }
        }
    }

    let raw = image_bytes.ok_or_else(|| AppError::MissingField("file".into()))?;
    let entity = entity.ok_or_else(|| AppError::MissingField("entity".into()))?;
    let entity_id = entity_id.ok_or_else(|| AppError::MissingField("entity_id".into()))?;

    if entity.is_empty() || entity.contains('/') || entity.contains("..") {
        return Err(AppError::InvalidField("entity".into()));
    }
    if entity_id.is_empty() || entity_id.contains('/') || entity_id.contains("..") {
        return Err(AppError::InvalidField("entity_id".into()));
    }

    let original_size = raw.len();
    let preset = processor::config_for(&entity);
    let processed = processor::process(&raw, &preset)?;
    let processed_size = processed.bytes.len();

    let key = storage::r2::upload(&state, &processed.bytes, &entity, &entity_id).await?;
    let url = format!("{}/{}", state.cfg.r2_public_base_url, key);

    Ok(Json(UploadResponse {
        key,
        url,
        original_size,
        processed_size,
        width: processed.width,
        height: processed.height,
    }))
}
