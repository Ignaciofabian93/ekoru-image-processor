use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct Config {
    pub r2_account_id: String,
    pub r2_access_key_id: String,
    pub r2_secret_access_key: String,
    pub r2_bucket: String,
    pub r2_public_base_url: String,
    pub internal_token: String,
    pub max_upload_bytes: usize,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let max_upload_bytes = std::env::var("MAX_UPLOAD_BYTES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10 * 1024 * 1024);

        let cfg = Self {
            r2_account_id: required("R2_ACCOUNT_ID")?,
            r2_access_key_id: required("R2_ACCESS_KEY_ID")?,
            r2_secret_access_key: required("R2_SECRET_ACCESS_KEY")?,
            r2_bucket: required("R2_BUCKET")?,
            r2_public_base_url: required("R2_PUBLIC_BASE_URL")?
                .trim_end_matches('/')
                .to_string(),
            internal_token: required("INTERNAL_TOKEN")?,
            max_upload_bytes,
        };

        if cfg.internal_token.len() < 16 {
            anyhow::bail!("INTERNAL_TOKEN must be at least 16 characters long");
        }

        Ok(cfg)
    }
}

fn required(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("missing required env var: {key}"))
}
