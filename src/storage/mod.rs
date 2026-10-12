pub mod r2;

use aws_sdk_s3::Client as S3Client;

/// Where processed images go. Production writes to Cloudflare R2; tests swap
/// in a map so the handlers run without a network (IMG-2).
pub enum Store {
    R2(S3Client),
    #[cfg(test)]
    Memory(std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>),
}
