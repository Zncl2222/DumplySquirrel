# DumplySquirrel

Dockerized backup management system with a Rust/Axum backend, PostgreSQL config database, React dashboard, Nginx reverse proxy, backup worker, and cron scheduling.

## Local Start

```bash
cp .env.example .env
docker compose up --build
```

The dashboard is served by Nginx on `http://localhost` by default. The backend is available inside Docker Compose as `backend:3000` and is exposed through `/api`.

For TLS, place `fullchain.pem` and `privkey.pem` under `./certs`, set `ENABLE_TLS=true`, then run `docker compose up --build`.

## Local Dev

The dev stack keeps PostgreSQL in Docker, but runs the app tier in dev mode:

- frontend: Vite on `http://localhost:5173`
- backend: Axum via `cargo run` on `http://localhost:8000`
- postgres: exposed on `localhost:5432`
- dev database name: `dumply_squirrel_dev`

Start it with:

```bash
./scripts/dev-up.sh
```

Run it detached with:

```bash
./scripts/dev-up.sh -d
```

Stop it with:

```bash
./scripts/dev-down.sh
```

If you only want the database for local testing/debugging:

```bash
./scripts/dev-db-up.sh
```

Stop only the database:

```bash
./scripts/dev-db-down.sh
```

Reset the dev database volume completely:

```bash
./scripts/dev-db-reset.sh
```

Dev-specific ports and origins live in `.env` / `.env.example` under the `Local dev stack` section.
The dev stack is separate from production `docker-compose.yml`, so it will not bind the Nginx `80/443` ports unless you start the production stack explicitly.
The frontend container will auto-run `npm ci` only when `node_modules` is missing.
In dev, the browser talks to Vite on `:5173`, and Vite proxies `/api` to the backend container internally.

## Contributing

This project uses GitHub Flow: branch from `main`, open a pull request, keep CI green, then merge back to `main` after review.

See [CONTRIBUTING.md](CONTRIBUTING.md) for branch naming and local check commands.

## Host Dev

If you want more control while debugging, use Docker only for the dev database and run the app tier directly on your machine.

Start only the dev database:

```bash
./scripts/dev-db-up.sh
```

Run the backend locally with Cargo:

```bash
./scripts/dev-local-backend.sh
```

You can also run it directly from the backend directory after the dev database is up:

```bash
cd backend
cargo run
```

Run the frontend locally with npm:

```bash
./scripts/dev-local-frontend.sh npm
```

Run the frontend locally with bun:

```bash
./scripts/dev-local-frontend.sh bun
```

Or start backend and frontend together while keeping the DB in Docker:

```bash
./scripts/dev-local-up.sh npm
```

For bun:

```bash
./scripts/dev-local-up.sh bun
```

In host dev, the frontend still calls `/api`; Vite proxies that to `http://127.0.0.1:8000` by default.
In host dev, `cargo run` uses your machine's PATH, so PostgreSQL backups require a compatible `pg_dump`/`psql` and MySQL backups require `mysqldump`/`mysql` to be installed on the host. PostgreSQL auto mode detects the target server major version and only uses a matching `pg_dump`; install `postgresql-client-14` through `postgresql-client-18` if you need the same behavior as the Docker backend images. MySQL auto mode logs the target version but still uses the system `mysqldump`.
The host dev scripts stop the matching Docker dev app container first, so ports `8000` and `5173` are free for local `cargo` / `npm` / `bun` processes.

## Implemented

- PostgreSQL migrations for users, backup configs, and backup history.
- Bootstrap admin from `ADMIN_USERNAME` / `ADMIN_PASSWORD` when no users exist.
- JWT login, logout placeholder, and current-user endpoint.
- User list/create/delete for admin users.
- Backup config list/create/update/delete/toggle with encrypted `db_url` storage and masked API output.
- Manual trigger endpoint that starts a guarded background backup worker.
- PostgreSQL backup via version-matched `pg_dump` using `PGPASSWORD` instead of password arguments.
- MySQL backup via system `mysqldump` using a temporary option file instead of password arguments.
- Backup timeout handling, partial-file cleanup, success/failure history updates, and retention cleanup.
- Cron scheduler using 6-field schedules (`sec min hour day month weekday`) for enabled configs.
- Backup download streaming with canonical `BACKUP_DIR` path checks.
- Config deletion blocks running backups and best-effort cleans up existing backup files.
- Dashboard stats endpoint.
- React dashboard for login, stats, backup config CRUD, manual trigger, history download, and user management.
- Nginx reverse proxy serving the frontend and proxying `/api` to the backend.
- Optional TLS mode controlled by `ENABLE_TLS`.
- Backend graceful shutdown and configurable CORS origin.

## Pending

- Automated integration tests against live PostgreSQL/MySQL targets.
- Backup cancellation endpoint.
