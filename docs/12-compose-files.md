# `compose.yml`, `compose.staging.yml`, `compose.prod.yml`

## What these files are

Three Docker Compose files that describe how to run the service in each environment. They share a structure but differ in three important ways:

| File                  | Used for          | Network                    | Port exposed?  |
| --------------------- | ----------------- | -------------------------- | -------------- |
| `compose.yml`         | Local development | (default bridge)           | ✅ 8090 → host |
| `compose.staging.yml` | Staging server    | `ekoru-staging-network`    | ❌ private only |
| `compose.prod.yml`    | Production server | `ekoru-network`            | ❌ private only |

The "port exposed?" distinction is the key one for security: in staging/prod, the service is **only** reachable from inside the docker network. The gateway is on that same network and can call it by container DNS name (`ekoru-image-processor-staging:8090`); anything outside the network — including someone with shell access to the host — cannot.

## `compose.yml` — local dev

```yaml
name: 'ekoru-image-processor-dev'

services:
  ekoru-image-processor:
    image: ekoru-image-processor:dev
    build:
      context: .
      dockerfile: Dockerfile
    container_name: ekoru-image-processor-dev
    restart: unless-stopped
    env_file:
      - .env
    ports:
      - '8090:8090'
    healthcheck:
      test: ['CMD', 'wget', '--quiet', '--tries=1', '--spider', 'http://localhost:8090/health']
      interval: 30s
      timeout: 5s
      retries: 3
      start_period: 15s
```

### Walkthrough

```yaml
name: 'ekoru-image-processor-dev'
```

The project name. Compose uses it as a prefix for volumes, networks, and the default network it creates. Visible in `docker compose ls`.

```yaml
services:
  ekoru-image-processor:
```

One service called `ekoru-image-processor`. The service name is also the **DNS name** other containers on the same Docker network use to reach it.

```yaml
    image: ekoru-image-processor:dev
    build:
      context: .
      dockerfile: Dockerfile
```

- `image:` — the tag the built image gets.
- `build.context: .` — paths in the `Dockerfile` are relative to this directory (project root).
- `build.dockerfile: Dockerfile` — which Dockerfile to use (default value, included for clarity).

When you `docker compose up`, it builds the image first if it doesn't exist, then runs it.

```yaml
    container_name: ekoru-image-processor-dev
    restart: unless-stopped
```

- `container_name` — fixed name (otherwise Compose generates `<project>-<service>-1`). Easier to `docker logs <name>`.
- `restart: unless-stopped` — auto-restart if the container crashes or the host reboots, **unless** you explicitly stopped it with `docker compose stop`.

```yaml
    env_file:
      - .env
```

Loads environment variables from `.env`. Compose reads the file and passes the key=value pairs to the container at startup. Same source `dotenvy::dotenv()` would read locally, just via the Docker injection path.

```yaml
    ports:
      - '8090:8090'
```

`host:container` mapping. Port 8090 on your machine forwards to port 8090 in the container. **This is the part removed in staging/prod** — there, only the docker network can reach it.

```yaml
    healthcheck:
      test: ['CMD', 'wget', '--quiet', '--tries=1', '--spider', 'http://localhost:8090/health']
      interval: 30s
      timeout: 5s
      retries: 3
      start_period: 15s
```

Tells Docker how to know if the container is healthy.

- `test` — the command to run. `wget --spider` is a HEAD-only probe; `--quiet` suppresses output. The `['CMD', ...]` form runs the command directly without a shell (more efficient, fewer surprises).
- `interval: 30s` — check every 30 seconds.
- `timeout: 5s` — fail the check if it takes longer than 5s.
- `retries: 3` — three consecutive failures before marking unhealthy.
- `start_period: 15s` — give the container 15s to start before we start counting failures (compiled Rust binaries start in <1s, so this is generous).

Health status is visible in `docker ps` (`(healthy)` / `(unhealthy)`). Orchestrators (Compose, Swarm, Kubernetes) can use it to decide whether to send traffic.

## `compose.staging.yml`

```yaml
name: 'ekoru-image-processor-staging'

services:
  ekoru-image-processor-staging:
    image: ekoru-image-processor:staging
    build:
      context: .
      dockerfile: Dockerfile
    restart: always
    container_name: ekoru-image-processor-staging
    env_file:
      - .env.staging
    networks:
      - ekoru-staging-network
    expose:
      - '8090'
    deploy:
      resources:
        limits:
          cpus: '1.0'
          memory: 512M
        reservations:
          cpus: '0.25'
          memory: 256M
    healthcheck:
      test: ['CMD', 'wget', '--quiet', '--tries=1', '--spider', 'http://localhost:8090/health']
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 40s

networks:
  ekoru-staging-network:
    external: true
```

### What's different from `compose.yml`

| Setting                  | Dev                            | Staging                                                                                                    |
| ------------------------ | ------------------------------ | ---------------------------------------------------------------------------------------------------------- |
| `container_name`         | `…-dev`                        | `…-staging` (matches the DNS name the gateway uses)                                                        |
| `restart`                | `unless-stopped`               | `always` — restart even if you stopped the container manually. Production assumption: if it's stopped, that's a bug. |
| `env_file`               | `.env`                         | `.env.staging`                                                                                             |
| `ports`                  | `8090:8090`                    | **(removed)** — no host-to-container mapping                                                                |
| `expose`                 | (not used)                     | `8090` — documents the internal port. Doesn't publish it to the host; only makes it explicit to humans/tools. |
| `networks`               | (default bridge)               | `ekoru-staging-network` — same external network the gateway joins                                          |
| `deploy.resources`       | (none)                         | CPU/memory limits and reservations                                                                          |
| `healthcheck.start_period` | 15s                          | 40s — generous because cold image-pull + container start on a shared host takes longer                      |

### Networks

```yaml
networks:
  ekoru-staging-network:
    external: true
```

`external: true` means **this compose file does not create the network — it assumes it already exists.** Both this service and the gateway need to be on it; whichever you start first runs into the network only if it was pre-created on the host.

That's why the README and the file header tell you:

```bash
docker network create ekoru-staging-network || true
```

`|| true` makes "already exists" a non-error.

Once both services are on the network, the gateway's NestJS code can `fetch("http://ekoru-image-processor-staging:8090/process", ...)` and Docker's embedded DNS resolves the name to the container's IP.

### Resource limits

```yaml
deploy:
  resources:
    limits:
      cpus: '1.0'
      memory: 512M
    reservations:
      cpus: '0.25'
      memory: 256M
```

- `limits` — hard ceiling. The container is killed if it exceeds memory; throttled if it exceeds CPU.
- `reservations` — soft floor. The scheduler tries to guarantee at least this much when placing the container.

512 MB is generous for what this service does — even processing large images, peak memory is <100 MB. The limit is a safety net against runaway allocations.

> **About `deploy:`** — these settings are honored by **Swarm** and **`docker compose --compatibility`** (which translates them to plain `docker run` flags). With plain `docker compose up` (no `--compatibility`), they're often **ignored**. For consistent enforcement, use `mem_limit:` and `cpus:` at the service level instead. We kept the `deploy:` shape to mirror the gateway's compose files.

## `compose.prod.yml`

Same shape as staging, with these differences:

| Setting          | Staging                              | Prod                            |
| ---------------- | ------------------------------------ | ------------------------------- |
| `name`           | `ekoru-image-processor-staging`      | `ekoru-image-processor-prod`    |
| `container_name` | `ekoru-image-processor-staging`      | `ekoru-image-processor`         |
| `image` tag      | `:staging`                           | `:prod`                         |
| `env_file`       | `.env.staging`                       | `.env.prod`                     |
| `networks`       | `ekoru-staging-network`              | `ekoru-network`                 |
| CPU limit        | 1.0 / reservation 0.25               | 2.0 / reservation 0.5           |
| Memory limit     | 512M / reservation 256M              | 1024M / reservation 512M        |

Production gets more CPU and memory because (a) more concurrent uploads and (b) running tighter to a limit risks more cold-restart events.

## How the gateway reaches the service

The gateway's compose file declares the **same** network as `external: true`. When both services come up, they share the network and resolve each other by container name.

In the gateway's NestJS code:

```ts
// Staging
const IMAGE_PROCESSOR_URL = 'http://ekoru-image-processor-staging:8090';

// Prod
const IMAGE_PROCESSOR_URL = 'http://ekoru-image-processor:8090';
```

(The exact env-var-driven shape is in [the README's gateway integration section](../README.md#integrating-with-the-gateway).)

This is the **only** way to reach the service in staging/prod. There's no port mapping to the host, no public DNS. The only authentication needed is the `X-Internal-Token` we already covered in [06-src-auth.md](06-src-auth.md).

## Common changes

| You want to…                                                | What to do                                                                                  |
| ----------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| Run multiple replicas (for HA)                              | Replace `container_name:` with `deploy.replicas: 2`. Compose can't load-balance on its own — put a reverse proxy in front, or use Swarm. |
| Mount a host directory for logs                              | Add `volumes: ['/var/log/ekoru-image-processor:/app/logs']` and reconfigure tracing to write to a file. (Currently we just log to stderr, and `docker logs` is the source of truth — easier.) |
| Stop / restart only this service                            | `docker compose -f compose.staging.yml restart`.                                            |
| Tail logs                                                    | `docker compose -f compose.staging.yml logs -f`.                                            |
| Verify config after env-var changes                          | `docker compose -f compose.staging.yml config` — prints the resolved YAML with env vars expanded. |
| Stop publishing to the host even in dev                      | Remove the `ports:` block from `compose.yml`. Local testing becomes "exec into the container" only. |
