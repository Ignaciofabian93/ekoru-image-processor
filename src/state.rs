use std::sync::Arc;

use anyhow::Result;
use aws_config::BehaviorVersion;
use aws_sdk_s3::{
    config::{Credentials, Region},
    Client as S3Client,
};

use crate::{config::Config, storage::Store};

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub cfg: Arc<Config>,
}

impl AppState {
    pub async fn new(cfg: Config) -> Result<Self> {
        let creds = Credentials::new(
            cfg.r2_access_key_id.clone(),
            cfg.r2_secret_access_key.clone(),
            None,
            None,
            "r2",
        );

        let s3_cfg = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("auto"))
            .endpoint_url(format!(
                "https://{}.r2.cloudflarestorage.com",
                cfg.r2_account_id
            ))
            .credentials_provider(creds)
            .build();

        Ok(Self {
            store: Arc::new(Store::R2(S3Client::from_conf(s3_cfg))),
            cfg: Arc::new(cfg),
        })
    }

    /// State backed by an in-memory store, for handler tests.
    #[cfg(test)]
    pub fn in_memory(cfg: Config) -> Self {
        Self {
            store: Arc::new(Store::Memory(Default::default())),
            cfg: Arc::new(cfg),
        }
    }
}
