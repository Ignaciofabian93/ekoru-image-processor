use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};

use crate::{error::AppError, state::AppState};

pub async fn require_internal_token(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let presented = req
        .headers()
        .get("x-internal-token")
        .and_then(|v| v.to_str().ok());

    match presented {
        Some(tok) if constant_time_eq(tok.as_bytes(), state.cfg.internal_token.as_bytes()) => {
            Ok(next.run(req).await)
        }
        _ => Err(AppError::Unauthorized),
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::constant_time_eq;

    #[test]
    fn compares_whole_tokens() {
        assert!(constant_time_eq(b"secret-token-1234", b"secret-token-1234"));
        assert!(!constant_time_eq(
            b"secret-token-1234",
            b"secret-token-1235"
        ));
        assert!(!constant_time_eq(b"secret-token-1234", b"secret-token-123"));
        assert!(!constant_time_eq(b"", b"x"));
    }
}
