use axum::{
    extract::DefaultBodyLimit,
    middleware,
    routing::{delete, get, post},
    Router,
};
use tower_http::trace::TraceLayer;

use crate::{auth, handlers, state::AppState};

/// The HTTP surface: `/health` open, everything else behind the internal
/// token. Built here rather than in `main` so tests can drive it (IMG-2).
pub fn router(state: AppState) -> Router {
    let body_limit = state.cfg.max_upload_bytes + 1024 * 1024;

    let protected = Router::new()
        .route("/process", post(handlers::upload::handle_upload))
        .route("/objects/{*key}", delete(handlers::delete::handle_delete))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_internal_token,
        ));

    Router::new()
        .route("/health", get(|| async { "ok" }))
        .merge(protected)
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::router;
    use crate::{
        config::Config,
        processor::tests::{png, png_claiming},
        state::AppState,
        storage::Store,
    };

    const TOKEN: &str = "test-token-0123456789";
    const BOUNDARY: &str = "ekoru-test-boundary";

    fn state(max_upload_bytes: usize) -> AppState {
        AppState::in_memory(Config {
            r2_account_id: "account".into(),
            r2_access_key_id: "key".into(),
            r2_secret_access_key: "secret".into(),
            r2_bucket: "bucket".into(),
            r2_public_base_url: "https://cdn.example".into(),
            internal_token: TOKEN.into(),
            max_upload_bytes,
        })
    }

    fn stored(state: &AppState) -> Vec<String> {
        match &*state.store {
            Store::Memory(map) => map.lock().unwrap().keys().cloned().collect(),
            Store::R2(_) => unreachable!("tests use the memory store"),
        }
    }

    /// A multipart body with the given text fields and an optional file.
    fn multipart(fields: &[(&str, &str)], file: Option<&[u8]>) -> Vec<u8> {
        let mut body = Vec::new();
        for (name, value) in fields {
            body.extend_from_slice(
                format!(
                    "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
                )
                .as_bytes(),
            );
        }
        if let Some(bytes) = file {
            body.extend_from_slice(
                format!(
                    "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\n"
                )
                .as_bytes(),
            );
            body.extend_from_slice(bytes);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
        body
    }

    fn upload(token: Option<&str>, body: Vec<u8>) -> Request<Body> {
        let mut req = Request::post("/process").header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        );
        if let Some(token) = token {
            req = req.header("x-internal-token", token);
        }
        req.body(Body::from(body)).unwrap()
    }

    async fn send(state: &AppState, req: Request<Body>) -> (StatusCode, Value) {
        let res = router(state.clone()).oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn health_needs_no_token() {
        let res = router(state(1024))
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn refuses_a_missing_or_wrong_token() {
        let st = state(1 << 20);
        let body = || {
            multipart(
                &[("entity", "product"), ("entity_id", "42")],
                Some(&png(10, 10)),
            )
        };
        assert_eq!(
            send(&st, upload(None, body())).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            send(&st, upload(Some("test-token-0123456780"), body()))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert!(stored(&st).is_empty());
    }

    #[tokio::test]
    async fn processes_and_stores_an_upload() {
        let st = state(8 << 20);
        let body = multipart(
            &[("entity", "product"), ("entity_id", "42")],
            Some(&png(1600, 800)),
        );
        let (status, json) = send(&st, upload(Some(TOKEN), body)).await;
        assert_eq!(status, StatusCode::OK, "{json}");

        let key = json["key"].as_str().unwrap();
        assert!(
            key.starts_with("product/42/") && key.ends_with(".webp"),
            "{key}"
        );
        assert_eq!(json["url"], format!("https://cdn.example/{key}"));
        assert_eq!(
            (json["width"].as_u64(), json["height"].as_u64()),
            (Some(800), Some(400))
        );
        assert_eq!(stored(&st), vec![key.to_string()]);
    }

    #[tokio::test]
    async fn rejects_missing_and_unsafe_fields() {
        let st = state(1 << 20);
        let image = png(10, 10);
        type Case<'a> = (&'a [(&'a str, &'a str)], Option<&'a [u8]>);
        let cases: [Case; 4] = [
            (&[("entity", "product"), ("entity_id", "42")], None),
            (&[("entity_id", "42")], Some(&image)),
            (&[("entity", "../etc"), ("entity_id", "42")], Some(&image)),
            (&[("entity", "product"), ("entity_id", "a/b")], Some(&image)),
        ];
        for (fields, file) in cases {
            let (status, json) = send(&st, upload(Some(TOKEN), multipart(fields, file))).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{fields:?}: {json}");
        }
        assert!(stored(&st).is_empty());
    }

    #[tokio::test]
    async fn rejects_a_file_over_the_byte_limit() {
        let st = state(1024);
        let body = multipart(
            &[("entity", "product"), ("entity_id", "1")],
            Some(&vec![0u8; 2048]),
        );
        assert_eq!(
            send(&st, upload(Some(TOKEN), body)).await.0,
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }

    #[tokio::test]
    async fn rejects_a_decompression_bomb_with_400() {
        let st = state(1 << 20);
        let body = multipart(
            &[("entity", "product"), ("entity_id", "1")],
            Some(&png_claiming(50_000)),
        );
        let (status, json) = send(&st, upload(Some(TOKEN), body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            json["error"]
                .as_str()
                .unwrap()
                .starts_with("image too large"),
            "{json}"
        );
        assert!(stored(&st).is_empty());
    }

    #[tokio::test]
    async fn rejects_bytes_that_are_not_an_image() {
        let st = state(1 << 20);
        let body = multipart(
            &[("entity", "product"), ("entity_id", "1")],
            Some(b"not an image"),
        );
        assert_eq!(
            send(&st, upload(Some(TOKEN), body)).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[tokio::test]
    async fn deletes_an_object_and_refuses_traversal() {
        let st = state(1 << 20);
        if let Store::Memory(map) = &*st.store {
            map.lock()
                .unwrap()
                .insert("product/42/a.webp".into(), vec![1]);
        }
        let del = |path: &str| {
            Request::delete(path)
                .header("x-internal-token", TOKEN)
                .body(Body::empty())
                .unwrap()
        };
        assert_eq!(
            send(&st, del("/objects/product/42/a.webp")).await.0,
            StatusCode::NO_CONTENT
        );
        assert!(stored(&st).is_empty());
        assert_eq!(
            send(&st, del("/objects/product/../secret")).await.0,
            StatusCode::BAD_REQUEST
        );
    }
}
