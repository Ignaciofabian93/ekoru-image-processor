# ekoru-image-processor

A small, internal Rust service that:

1. Accepts an image upload from the **ekoru-gateway** (NestJS).
2. **Resizes** it to a size appropriate for the entity it belongs to (avatar, product, cover, ...).
3. **Re-encodes** it as WebP.
4. **Uploads** it to a Cloudflare R2 bucket.
5. Returns the public URL back to the gateway, which stores it in the database.

The service is intentionally **not exposed to the public internet**. It lives on the same Docker network as the gateway and only the gateway calls it. Authentication between the two services is a shared secret in the `X-Internal-Token` header.

> 📖 **Per-file deep dives:** every source file, Dockerfile, compose file, and env file is explained in detail — libraries, idioms, line-by-line walkthroughs — under [`docs/`](docs/00-index.md). Read it if you're new to Rust or want to understand _why_ the code looks the way it does.

---

## Contents

- [Architecture](#architecture)
- [Project layout](#project-layout)
- [Cloudflare R2 — one-time setup](#cloudflare-r2--one-time-setup)
- [Environment variables](#environment-variables)
- [Local development](#local-development)
- [Deploying staging](#deploying-staging)
- [Deploying production](#deploying-production)
- [API reference](#api-reference)
- [Integrating with the gateway](#integrating-with-the-gateway)
- [How the Rust code is organized](#how-the-rust-code-is-organized)
- [Troubleshooting](#troubleshooting)

---

## Architecture

```
                ┌────────────┐
   client ───▶  │  Gateway   │  ── multipart upload
  (mobile/web)  │  (NestJS)  │     POST /api/profile-image
                │  :4000     │
                └─────┬──────┘
                      │  multipart + X-Internal-Token
                      │  POST http://ekoru-image-processor:8090/process
                      ▼
                ┌──────────────────────┐
                │  Image processor     │
                │  (Rust + axum)       │
                │  :8090 (private)     │
                └─────┬────────────────┘
                      │  S3 API (PutObject)
                      │  https://<account>.r2.cloudflarestorage.com
                      ▼
                ┌──────────────────────┐
                │   Cloudflare R2      │
                │   bucket per env     │
                └─────┬────────────────┘
                      │  served publicly via custom domain
                      ▼
                  https://cdn.ekoru.cl/<key>
```

**Per environment** there is one bucket and one custom-domain CDN URL:

| Env     | Bucket                  | Public URL                    | Internal DNS name                  |
| ------- | ----------------------- | ----------------------------- | ---------------------------------- |
| dev     | `ekoru-images-staging`  | `https://cdn-staging.ekoru.cl`| `localhost:8090` (host)            |
| staging | `ekoru-images-staging`  | `https://cdn-staging.ekoru.cl`| `ekoru-image-processor-staging:8090` |
| prod    | `ekoru-images-prod`     | `https://cdn.ekoru.cl`        | `ekoru-image-processor:8090`       |

---

## Project layout

```
ekoru-image-processor/
├── Cargo.toml              # Rust dependencies (like package.json)
├── Cargo.lock              # Locked dependency versions (commit this!)
├── Dockerfile              # Multi-stage build: builder → runtime
├── .dockerignore
├── .gitignore              # Excludes .env*, target/, IDE folders
├── .env.example            # Template — committed
├── .env                    # Local dev — gitignored
├── .env.staging            # Staging server — gitignored
├── .env.prod               # Prod server — gitignored
├── compose.yml             # Dev compose (exposes port locally)
├── compose.staging.yml     # Staging compose (private network only)
├── compose.prod.yml        # Prod compose (private network only)
├── README.md               # This file
└── src/
    ├── main.rs             # Entrypoint: wires the router + middleware
    ├── config.rs           # Loads + validates env vars
    ├── state.rs            # Shared state (S3 client, config) injected per request
    ├── error.rs            # AppError enum + HTTP response mapping
    ├── auth.rs             # X-Internal-Token middleware
    ├── handlers/
    │   ├── mod.rs
    │   ├── upload.rs       # POST /process
    │   └── delete.rs       # DELETE /objects/{key}
    ├── processor/
    │   └── mod.rs          # Resize + WebP encode logic
    └── storage/
        ├── mod.rs
        └── r2.rs           # S3-compatible calls to R2
```

---

## Cloudflare R2 — one-time setup

You said you already created the two buckets. The remaining steps:

### 1. Get your **Account ID**

Cloudflare dashboard → top-right of the page. It's a 32-character hex string. Same for both environments.

### 2. Create an **R2 API token** (do this twice — once per env)

Dashboard → **R2** → **Manage R2 API Tokens** → **Create API token**:

- **Token name:** `ekoru-image-processor-staging` (then again `…-prod`)
- **Permissions:** _Object Read & Write_
- **Specify bucket:** select the corresponding bucket (one token per bucket scoped narrowly is safer than one wildcard token)
- **TTL:** "Forever" — or set a calendar reminder to rotate

Cloudflare will then show, **once**:

- **Access Key ID**
- **Secret Access Key**

Copy both immediately into the matching `.env.staging` / `.env.prod`. There is no way to retrieve the secret later — if you lose it, you create a new token and update the env files.

### 3. Connect a **custom domain** to each bucket

Dashboard → **R2** → click the bucket → **Settings** tab → under **Public access**, click **Connect Domain**.

- Staging bucket → `cdn-staging.ekoru.cl`
- Prod bucket → `cdn.ekoru.cl`

(If `ekoru.cl` is managed by Cloudflare DNS, this is a one-click flow that creates the CNAME record for you. If not, you'll need to add the CNAME manually in your DNS provider.)

The URL you connect here is exactly what goes into `R2_PUBLIC_BASE_URL`.

> **Why a custom domain instead of the `pub-<hash>.r2.dev` URL?**
> The `r2.dev` URL is rate-limited and not meant for production. Custom domains are unlimited and let you front the bucket with Cloudflare's cache + WAF.

---

## Environment variables

All variables live in a `.env` file. The service refuses to start if any required one is missing or if `INTERNAL_TOKEN` is shorter than 16 chars.

| Variable                | Required | Example                              | What it is                                                                           |
| ----------------------- | -------- | ------------------------------------ | ------------------------------------------------------------------------------------ |
| `R2_ACCOUNT_ID`         | ✅       | `abc123...xyz` (32 hex chars)        | Cloudflare account ID. Used to build the S3 endpoint URL.                            |
| `R2_ACCESS_KEY_ID`      | ✅       | `f0e1d2c3...`                        | From "Create R2 API token".                                                          |
| `R2_SECRET_ACCESS_KEY`  | ✅       | `b7a8...`                            | From "Create R2 API token". Shown once — store it now.                               |
| `R2_BUCKET`             | ✅       | `ekoru-images-staging`               | Bucket name (not the URL).                                                           |
| `R2_PUBLIC_BASE_URL`    | ✅       | `https://cdn-staging.ekoru.cl`       | Custom domain attached to the bucket. No trailing slash.                             |
| `INTERNAL_TOKEN`        | ✅       | `b8c9...` (≥ 16 chars)               | Shared secret with the gateway. **Generate with `openssl rand -hex 32`.**            |
| `MAX_UPLOAD_BYTES`      | ❌       | `10485760` (10 MiB, default)         | Hard limit on raw upload size before resize.                                         |
| `RUST_LOG`              | ❌       | `info,aws=warn,hyper=warn` (default) | Log filter. See [`tracing-subscriber` docs](https://docs.rs/tracing-subscriber).     |

### Where to put values

| Env     | File                                     | Where it's loaded                              |
| ------- | ---------------------------------------- | ---------------------------------------------- |
| dev     | `.env`                                   | `dotenvy::dotenv()` in `main.rs`               |
| staging | `.env.staging`                           | `env_file:` in `compose.staging.yml`           |
| prod    | `.env.prod`                              | `env_file:` in `compose.prod.yml`              |

All three files are **gitignored**. `.env.example` is the only committed template.

### Sharing the token with the gateway

The gateway needs the **same** `INTERNAL_TOKEN` value for each environment. Once you finalize the value here, add a matching variable to the gateway's env files (for example `IMAGE_PROCESSOR_TOKEN=...`) and use it when the gateway calls this service.

---

## Local development

### Prerequisites

- **Rust** ≥ 1.85. Install via [rustup](https://rustup.rs/). Verify with `rustc --version`. (1.85 is required because a transitive dep — `ravif`, pulled in by `image` — needs Cargo edition-2024 support.)
- **Docker Desktop** (if you want to run with compose).
- A populated `.env` file (see above).

### Option A — run directly with cargo (fastest dev loop)

```powershell
# From the project root (c:\Dev\ekoru-repos\ekoru-image-processor)
cargo run
```

First run downloads ~200 crates and takes 2–5 min. Subsequent runs only recompile what changed.

When you see:

```
INFO ekoru_image_processor: listening on 0.0.0.0:8090
```

…hit it:

```powershell
# Health check (no auth required)
curl http://localhost:8090/health

# Real upload (replace INTERNAL_TOKEN with what's in your .env)
curl -X POST http://localhost:8090/process `
  -H "X-Internal-Token: dev-internal-token-please-change-me" `
  -F "file=@C:\path\to\photo.jpg" `
  -F "entity=user_avatar" `
  -F "entity_id=42"
```

Successful response (formatted):

```json
{
  "key": "user_avatar/42/9f2a...abcd.webp",
  "url": "https://cdn-staging.ekoru.cl/user_avatar/42/9f2a...abcd.webp",
  "original_size": 1485322,
  "processed_size": 23104,
  "width": 400,
  "height": 400
}
```

### Option B — run with docker compose

```powershell
docker compose up --build
```

This builds the image (slow first time, ~3–6 min) and starts the container with port 8090 exposed locally. Use the same curl commands as above.

### Useful cargo commands

| Command           | What it does                                              |
| ----------------- | --------------------------------------------------------- |
| `cargo run`       | Compile + run (debug build, fast to compile, slow at run) |
| `cargo build`     | Compile only                                              |
| `cargo build --release` | Compile with optimizations (used by Dockerfile)      |
| `cargo check`     | Type-check without producing a binary (very fast)         |
| `cargo fmt`       | Format the code                                           |
| `cargo clippy`    | Lint                                                      |

---

## Deploying staging

On the staging server (the same host that runs `ekoru-gateway-staging`):

```bash
# 1. Clone or pull this repo
git pull

# 2. Ensure the env file exists and has real values (NOT REPLACE_ME)
cat .env.staging       # sanity-check

# 3. Make sure the shared network exists (only needed once per host)
docker network create ekoru-staging-network || true

# 4. Build + start
docker compose -f compose.staging.yml up -d --build

# 5. Watch the logs
docker compose -f compose.staging.yml logs -f
```

The service is now reachable from any other container on `ekoru-staging-network` at:

```
http://ekoru-image-processor-staging:8090
```

It is **not** reachable from outside the host. There is no `ports:` mapping in the staging compose file by design.

---

## Deploying production

Same as staging, swapping the file name:

```bash
git pull
cat .env.prod
docker network create ekoru-network || true
docker compose -f compose.prod.yml up -d --build
docker compose -f compose.prod.yml logs -f
```

Internal DNS: `http://ekoru-image-processor:8090`.

---

## API reference

All endpoints under `/process` and `/objects/*` require this header:

```
X-Internal-Token: <the INTERNAL_TOKEN env var>
```

Missing or wrong header → `401 Unauthorized`.

### `GET /health`

No auth. Returns `200 OK` with body `ok`. Used by the Docker `healthcheck`.

### `POST /process`

Receives a multipart upload, resizes the image, encodes as WebP, uploads to R2.

**Headers**

| Header             | Required | Value                    |
| ------------------ | -------- | ------------------------ |
| `X-Internal-Token` | ✅       | shared secret            |
| `Content-Type`     | ✅       | `multipart/form-data; …` |

**Form fields**

| Field       | Type   | Required | Notes                                                                                  |
| ----------- | ------ | -------- | -------------------------------------------------------------------------------------- |
| `file`      | file   | ✅       | The raw image. JPEG, PNG, WebP, GIF, BMP, TIFF accepted.                               |
| `entity`    | text   | ✅       | One of `user_avatar`, `user_cover`, `product`, `service`, `community`, `asset`. Determines resize preset. Must not contain `/` or `..`. Unknown values fall back to 800×800. |
| `entity_id` | text   | ✅       | Stable ID of the owning record (e.g. user ID, product ID). Used as part of the object key. Must not contain `/` or `..`. |

**Response — 200**

```json
{
  "key": "product/123/0190b8c9-...-...-...-....webp",
  "url": "https://cdn.ekoru.cl/product/123/0190b8c9-...-...-...-....webp",
  "original_size": 2034512,
  "processed_size": 41024,
  "width": 800,
  "height": 600
}
```

**Errors**

| Status | When                                                  |
| ------ | ----------------------------------------------------- |
| 400    | Missing/invalid `file`, `entity`, or `entity_id` field |
| 401    | Missing or wrong `X-Internal-Token`                    |
| 413    | Upload exceeded `MAX_UPLOAD_BYTES`                     |
| 422    | The file wasn't a decodable image                      |
| 500    | R2 upload failed or unexpected server error            |

### Resize presets

Defined in [src/processor/mod.rs](src/processor/mod.rs):

| `entity`      | Max width × height |
| ------------- | ------------------ |
| `user_avatar` | 400 × 400          |
| `user_cover`  | 1200 × 400         |
| `product`     | 800 × 800          |
| `service`     | 800 × 600          |
| `community`   | 1000 × 700         |
| `asset`       | 1920 × 1080        |
| _(other)_     | 800 × 800          |

Aspect ratio is preserved — the image is scaled to fit _inside_ the box, not cropped.

### `DELETE /objects/{key}`

Removes an object from the bucket. Pass the `key` returned from a previous upload (URL-encoded so the `/` separators are preserved literally — most HTTP clients do this for you).

**Response — 204** on success (no body). Returns `204` even if the object didn't exist (R2 returns no error for missing keys on delete).

---

## Integrating with the gateway

You opted to wire up the gateway yourself, so here's the contract laid out plainly plus a paste-ready NestJS service.

### High-level flow (per upload)

1. Client `POST`s a multipart to a gateway controller (e.g. `POST /api/profile-image`).
2. The gateway extracts the `Buffer` from `Express.Multer.File` exactly as today.
3. Instead of `imagesService.saveFile(buffer, ...)`, the gateway calls this service:
   ```
   POST http://<image-processor-host>:8090/process
   Headers: X-Internal-Token: <IMAGE_PROCESSOR_TOKEN>
   Body (multipart):
     file=<buffer>
     entity=user_avatar
     entity_id=<sellerId>
   ```
4. The gateway gets `{ key, url, ... }` back and writes `url` (or `key` if you'd rather rebuild URLs at read time) to the database.

### Gateway env vars to add

In `ekoru-gateway/.env`, `.env.staging`, `.env.prod`:

```env
# dev
IMAGE_PROCESSOR_URL=http://localhost:8090
IMAGE_PROCESSOR_TOKEN=dev-internal-token-please-change-me

# staging
IMAGE_PROCESSOR_URL=http://ekoru-image-processor-staging:8090
IMAGE_PROCESSOR_TOKEN=<same value as image-processor .env.staging>

# prod
IMAGE_PROCESSOR_URL=http://ekoru-image-processor:8090
IMAGE_PROCESSOR_TOKEN=<same value as image-processor .env.prod>
```

### Drop-in NestJS client

Create `src/images/image-processor.client.ts` in the gateway:

```ts
import { Injectable, InternalServerErrorException } from '@nestjs/common';
import { ConfigService } from '@nestjs/config';

export type ImageEntity =
  | 'user_avatar'
  | 'user_cover'
  | 'product'
  | 'service'
  | 'community'
  | 'asset';

export interface ProcessedImage {
  key: string;
  url: string;
  original_size: number;
  processed_size: number;
  width: number;
  height: number;
}

@Injectable()
export class ImageProcessorClient {
  private readonly baseUrl: string;
  private readonly token: string;

  constructor(config: ConfigService) {
    this.baseUrl = config.getOrThrow<string>('IMAGE_PROCESSOR_URL');
    this.token = config.getOrThrow<string>('IMAGE_PROCESSOR_TOKEN');
  }

  async upload(
    file: Express.Multer.File,
    entity: ImageEntity,
    entityId: string,
  ): Promise<ProcessedImage> {
    const form = new FormData();
    form.append(
      'file',
      new Blob([file.buffer], { type: file.mimetype }),
      file.originalname,
    );
    form.append('entity', entity);
    form.append('entity_id', entityId);

    const res = await fetch(`${this.baseUrl}/process`, {
      method: 'POST',
      headers: { 'X-Internal-Token': this.token },
      body: form,
    });

    if (!res.ok) {
      const text = await res.text().catch(() => '');
      throw new InternalServerErrorException(
        `image-processor returned ${res.status}: ${text}`,
      );
    }

    return (await res.json()) as ProcessedImage;
  }

  async delete(key: string): Promise<void> {
    const res = await fetch(
      `${this.baseUrl}/objects/${key.split('/').map(encodeURIComponent).join('/')}`,
      {
        method: 'DELETE',
        headers: { 'X-Internal-Token': this.token },
      },
    );
    if (!res.ok && res.status !== 404) {
      throw new InternalServerErrorException(
        `image-processor delete returned ${res.status}`,
      );
    }
  }
}
```

Then in `images.module.ts`, add it to `providers:` and `exports:` and inject it wherever you currently call `imagesService.saveFile`. Replace this kind of code:

```ts
// before
const imagePath = await this.imagesService.saveFile(
  file.buffer,
  'profile-images',
  fileName,
);
// ... save imagePath to DB
```

with:

```ts
// after
const { url } = await this.imageProcessor.upload(file, 'user_avatar', userId);
// ... save url to DB
```

The mapping from your existing folders → image-processor entity:

| Existing folder           | Use entity     |
| ------------------------- | -------------- |
| `profile-images`          | `user_avatar`  |
| `cover-images`            | `user_cover`   |
| `product-images`          | `product`      |
| `business-images`         | `product` or `service` (based on `itemType`) |
| `departments`             | `asset`        |
| `users` (general)         | `user_avatar`  |

### Network wiring

For staging/prod the gateway container must be on the same docker network as the image processor. Your gateway compose files already declare `ekoru-staging-network` and `ekoru-network` as external networks. Both this service's compose files join those exact networks, so they'll resolve each other by container name.

---

## How the Rust code is organized

If you're new to Rust, this is the short tour. Each module is small and does one thing.

### `main.rs` — the wiring

```rust
let app = Router::new()
    .route("/health", get(...))
    .merge(protected)                         // /process + /objects/{*key}
    .layer(DefaultBodyLimit::max(body_limit)) // max request body
    .layer(TraceLayer::new_for_http())        // structured logging
    .with_state(state);                       // shared S3 client + config
```

`Router` is axum's HTTP router. `.route(...)` registers a path + verb. `.layer(...)` wraps the entire router in middleware (runs for every request). `.with_state(state)` makes `AppState` available to every handler via the `State<AppState>` extractor.

The "protected" sub-router is wrapped in `route_layer(middleware::from_fn_with_state(state.clone(), auth::require_internal_token))`, which means **only** routes added through it require the header. `/health` stays open.

### `state.rs` — shared state

```rust
pub struct AppState {
    pub s3: Arc<S3Client>,
    pub cfg: Arc<Config>,
}
```

`Arc<T>` is "atomically reference-counted pointer" — basically `Rc<T>` from C++/Swift terms, but thread-safe. axum clones the state for every request; `Arc` makes that clone cheap (just a pointer + atomic counter bump). The S3 client maintains a connection pool internally, so we want **one** instance shared across the whole app, not one per request.

### `config.rs` — env loading

`Config::from_env()` reads every variable, fails with a clear message if any required one is missing, validates the token length, and returns a `Result<Config>`. Called once at startup.

### `error.rs` — one error type, many HTTP statuses

`AppError` is an enum where each variant is a thing that can go wrong (`MissingField`, `PayloadTooLarge`, `Image`, ...). The `impl IntoResponse for AppError` block teaches axum how to turn each variant into an HTTP response with the right status code and a JSON body. Handlers just return `Result<Json<…>, AppError>` and let axum do the conversion.

`#[derive(thiserror::Error)]` generates `Display` / `Error` impls. The `#[from]` attribute makes `?` automatically convert (e.g. an `image::ImageError` from `image::load_from_memory` becomes an `AppError::Image`).

### `auth.rs` — middleware

A tiny async function that runs before every protected handler. Pulls the `X-Internal-Token` header, constant-time-compares it to the configured value, and either lets the request through or returns `AppError::Unauthorized` (which `IntoResponse` then turns into a `401`). Constant-time compare prevents timing attacks where an attacker could detect partial matches.

### `handlers/upload.rs` — the workhorse

```rust
pub async fn handle_upload(
    State(state): State<AppState>,   // gimme the shared state
    mut multipart: Multipart,        // gimme the multipart body
) -> Result<Json<UploadResponse>, AppError> { ... }
```

Loops through multipart fields, stashes `file` / `entity` / `entity_id`, validates them, runs `processor::process`, hands the bytes to `storage::r2::upload`, returns JSON. `?` after each fallible call short-circuits to an `Err` if anything fails.

### `processor/mod.rs` — resize + encode

Uses the `image` crate. `img.resize(...)` preserves aspect ratio by fitting the image _inside_ the box. The `image` crate's WebP encoder (currently) only writes lossless WebP, which still compresses very well for photos taken from cameras and far better than the JPEG-as-saved-in-DB approach you had before.

### `storage/r2.rs` — Cloudflare R2 via S3

R2 speaks the S3 API. We point the AWS SDK at R2's endpoint (`https://<account>.r2.cloudflarestorage.com`), region `"auto"`, with static credentials from the env. Then `put_object()` / `delete_object()` are exactly the calls you'd make against real S3. Object key format is `{entity}/{entity_id}/{uuid}.webp` — flat, predictable, and the UUID guarantees no collisions even if the same `entity_id` re-uploads.

---

## Troubleshooting

### `cargo build` fails with "could not compile" / linker errors

You probably don't have a C linker installed. On Windows install the **MSVC Build Tools** from <https://visualstudio.microsoft.com/visual-cpp-build-tools/> (select "Desktop development with C++"). Then rerun.

### Service refuses to start: `missing required env var: R2_ACCOUNT_ID`

The `.env` (or whichever env-file the container reads) doesn't have that variable set, or it's commented out, or the value is empty. Re-check the file path the container actually loaded — `docker compose -f compose.staging.yml config` will print the resolved config.

### Service refuses to start: `INTERNAL_TOKEN must be at least 16 characters long`

You left the value as `REPLACE_ME` or set a short one. Generate a real one:

```powershell
# Windows / PowerShell
[System.Convert]::ToBase64String((1..32 | %{[byte](Get-Random -Max 256)}))

# Mac/Linux/WSL
openssl rand -hex 32
```

### Uploads return `401 Unauthorized`

The gateway is sending the wrong (or no) `X-Internal-Token`. Confirm:

- Both services have the **same** value in their respective env files.
- The gateway is reading from the same env file you edited (`docker compose ... config | grep IMAGE_PROCESSOR_TOKEN`).

### Uploads return `413 Payload Too Large`

Raise `MAX_UPLOAD_BYTES`. Remember the docker network may also have its own request size limits if you put a reverse proxy in front of the gateway.

### Uploads succeed, but `image.url` returns 403 / "Access Denied"

The custom domain isn't connected to the bucket yet, or the bucket's public access is disabled. Re-check **R2 → bucket → Settings → Public access**.

### Uploads succeed, but `image.url` returns 404

`R2_PUBLIC_BASE_URL` doesn't match the custom domain you connected. Compare exact strings (no trailing slash, correct subdomain).

### Service logs `dispatch failure` or `connection error` calling R2

Network/DNS issue from inside the container. Test:

```bash
docker compose -f compose.staging.yml exec ekoru-image-processor-staging \
  wget -qO- https://cloudflare.com
```

If that fails, the container's DNS is broken — usually a docker network config problem on the host.

### Build is slow

First build downloads + compiles the entire dependency tree (~200 crates). Subsequent rebuilds only recompile what changed, courtesy of the layer caching set up in the Dockerfile. If even rebuilds are slow, you may have invalidated the cache by editing `Cargo.toml`.
