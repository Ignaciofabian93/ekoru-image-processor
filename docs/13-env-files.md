# `.env`, `.env.staging`, `.env.prod`, `.env.example`

## What these files are

KEY=VALUE files holding every runtime knob the service needs. There are four:

| File              | When it's used                                                              | Committed?     |
| ----------------- | --------------------------------------------------------------------------- | -------------- |
| `.env.example`    | Reference / template. **Read** by humans only, never by the service.       | ✅ in git      |
| `.env`            | Local development. Read by `dotenvy::dotenv()` when you `cargo run` locally. | ❌ gitignored |
| `.env.staging`    | Staging server. Read by Docker via `env_file: .env.staging` in `compose.staging.yml`. | ❌ gitignored |
| `.env.prod`       | Production server. Read by Docker via `env_file: .env.prod` in `compose.prod.yml`. | ❌ gitignored |

`.env.example` is the **only** one safe to commit because it has no real values — just `REPLACE_ME` placeholders + explanations.

## Why the files are gitignored

Secrets in git stay in git **forever** — every clone, every fork, every backup retains them, even if you `git rm` and force-push later. The blast radius of accidentally committing prod R2 credentials is "rotate them immediately and audit the bucket for unauthorized access." Not worth the risk.

The `.gitignore` excludes everything starting with `.env*`, which catches:
- `.env`
- `.env.staging`
- `.env.prod`
- `.env.local`, `.env.development`, etc. — anything you might add later

`.env.example` is committed because its name follows the pattern but the file contains no secrets.

## Every variable, in depth

### `R2_ACCOUNT_ID`

Your Cloudflare account's identifier. **Not a secret per se** — it appears in any R2 URL you publish — but the AWS SDK needs it to construct the endpoint URL:

```
https://<R2_ACCOUNT_ID>.r2.cloudflarestorage.com
```

Where to find it: top-right of the Cloudflare dashboard, or in the URL of any R2 page. 32 hex characters.

Set the **same value** in `.env`, `.env.staging`, `.env.prod` (it's account-wide, not per-bucket).

### `R2_ACCESS_KEY_ID` and `R2_SECRET_ACCESS_KEY`

S3-compatible credentials. Generated when you click **R2 → Manage R2 API Tokens → Create API token** with **Object Read & Write** scope and a single bucket selected.

These **are** secrets. The secret access key is shown exactly **once** at creation time; if you lose it, you create a new token and update the env file.

Best practice: one token per environment, scoped to the matching bucket. That way a leaked staging key can't be used against prod.

| Env     | Token scope             |
| ------- | ----------------------- |
| staging | `ekoru-images-staging`  |
| prod    | `ekoru-images-prod` (or whatever you named yours) |

For local dev, point at the staging bucket and use the staging token. Never let dev write to prod.

### `R2_BUCKET`

The bucket name (just the name, no URL). Different per environment.

| Env     | Suggested name         |
| ------- | ---------------------- |
| dev     | `ekoru-images-staging` (use the staging bucket) |
| staging | `ekoru-images-staging` |
| prod    | `ekoru-images`         |

If you've already created different names, use what you've actually got. The service doesn't care about the naming convention.

### `R2_PUBLIC_BASE_URL`

The HTTPS URL where uploaded images can be fetched by browsers. Must be a **custom domain** you've attached to the bucket (R2 → bucket → Settings → Public access → Connect Domain).

| Env     | Example                          |
| ------- | -------------------------------- |
| dev     | `https://staging-images.ekoru.cl` (whatever staging uses) |
| staging | `https://staging-images.ekoru.cl` |
| prod    | `https://images.ekoru.cl`        |

No trailing slash. The code in [`config.rs`](../src/config.rs) trims it defensively if you include one, but the convention is "no trailing slash."

> **Why not use the `r2.dev` default URL?** It's rate-limited and explicitly marked "for development use only" by Cloudflare. For production traffic, always front the bucket with a custom domain — gets you unlimited bandwidth, CDN caching, and the ability to put WAF rules in front.

### `INTERNAL_TOKEN`

The shared secret between this service and the gateway. **This is a secret.**

- Must be at least **16 characters** or the service refuses to start (sanity check against placeholders like `dev-secret`).
- Generate one per environment with `openssl rand -hex 32` (64 chars of hex).
- The **same value** must appear in the gateway's matching env file (probably as `IMAGE_PROCESSOR_TOKEN`).

To rotate: pick a window, update both env files, restart both services. Currently the auth middleware accepts a single token — if you want zero-downtime rotation, modify it to accept multiple (see "Common changes" in [06-src-auth.md](06-src-auth.md)).

### `MAX_UPLOAD_BYTES` _(optional)_

Maximum size of an incoming image upload, in bytes. Default `10485760` (10 MiB).

If you want to allow bigger:

```env
MAX_UPLOAD_BYTES=20971520   # 20 MiB
```

Take into account:

- The router-level body limit in [`main.rs`](../src/main.rs) is `MAX_UPLOAD_BYTES + 1 MiB` (headroom for multipart overhead).
- The gateway has its own limit (`MulterModule.register({ limits: { fileSize: 5 * 1024 * 1024 }})` currently — 5 MiB). The smaller of the two wins, so raising it here without raising it in the gateway has no effect.

### `RUST_LOG` _(optional)_

Standard [`tracing-subscriber` env-filter](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html) syntax. Default `info,aws=warn,hyper=warn`.

Useful patterns:

| Value                                                | Effect                                                          |
| ---------------------------------------------------- | --------------------------------------------------------------- |
| `info`                                               | Info-and-above everywhere. Noisy because the AWS SDK logs a lot at info. |
| `info,aws=warn,hyper=warn`                           | Default. Quiet AWS/hyper, info from us.                          |
| `debug,ekoru_image_processor=trace,aws=warn`         | Verbose us, normal everything else. Good for local debugging.    |
| `warn,ekoru_image_processor=info`                    | Production: only warnings globally, info from us.                |

## A note on Windows paths

If you're editing the dev `.env` and any value has a Windows path (none currently), avoid backslashes — they're treated as escapes by some env parsers. Either use forward slashes (`C:/Users/...`) or escape backslashes (`C:\\Users\\...`).

`dotenvy` itself handles backslashes correctly, but it's a footgun worth knowing about.

## How env vars actually reach the running service

### Local dev (`cargo run`)

```
.env  ──→  dotenvy::dotenv()  ──→  std::env  ──→  Config::from_env() reads it
```

`dotenvy::dotenv()` runs at the top of `main`. It finds `.env` in the current working directory (or any ancestor up to the filesystem root) and copies its KEY=VALUE pairs into the process's environment.

### Local docker (`docker compose up`)

```
.env  ──→  Compose reads `env_file: .env`  ──→  passes as `docker run -e KEY=VALUE`  ──→  std::env  ──→  Config::from_env()
```

Here, `dotenvy::dotenv()` finds no `.env` inside the container, returns `Err`, we call `.ok()` to discard it, and the variables are already in `std::env` because Docker put them there.

Both paths converge on the same final state: every required `R2_*` and `INTERNAL_TOKEN` value in `std::env::var(...)`.

## Common changes

| You want to…                                                       | What to do                                                                                  |
| ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------- |
| Rotate R2 credentials                                              | Cloudflare dashboard → new API token → update `.env.<env>` → restart container.              |
| Use a secret manager instead of `.env`                              | Replace the `env_file:` entry in compose with `environment:` and have an init script render the values from your secret manager. Or run docker with `--env-file <(secret-manager-cmd)`. |
| Override one variable for a quick test                              | `R2_BUCKET=different-bucket cargo run` (PowerShell: `$env:R2_BUCKET="different-bucket"; cargo run`). Process-level vars take precedence over `.env`. |
| Confirm what the container actually sees                            | `docker compose -f compose.staging.yml exec ekoru-image-processor-staging printenv \| sort`. |
| Add a new variable                                                  | Add a field to `Config`, read it in `from_env`, document it in `.env.example`, then add the real value to each `.env*`. |
