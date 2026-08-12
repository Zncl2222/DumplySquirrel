# DumplySquirrel

Dockerized backup management system with a Rust/Axum backend, PostgreSQL config database, React dashboard, Nginx reverse proxy, backup worker, and cron scheduling.

## Local Start

```bash
./scripts/init-env.sh
docker compose up --build
```

The production backend rejects the public `change_me_*` placeholders and secrets shorter than
their minimum safe length. The development scripts and dev Compose stack expose
`DEV_ALLOW_INSECURE_SECRETS=true` as an explicit, off-by-default development override; production
Compose does not pass that override. Development ports bind to localhost by default.

`scripts/init-env.sh` refuses to overwrite an existing `.env`, generates URL-safe random secrets,
creates it atomically with mode `0600`, and stores the first-login credentials in
`ADMIN_USERNAME` / `ADMIN_PASSWORD` inside that file.

For an existing installation, rotate secrets deliberately:

- Changing `DB_PASSWORD` also requires changing the PostgreSQL role password. Leave
  `DB_PASSWORD_URL_ENCODED` blank for URL-safe values; if the password contains URL-reserved
  characters, set that second variable to the percent-encoded form used in `DATABASE_URL`.
- Changing `JWT_SECRET` signs out every existing session.
- To rotate field encryption, put the new key in `DATABASE_ENCRYPTION_KEY` and the old key in
  `DATABASE_ENCRYPTION_KEY_PREVIOUS`, start the backend once, then remove the previous-key value.
  Rotation locks and updates all saved target URLs in one database transaction.
- To reset an existing bootstrap account, set the new `ADMIN_PASSWORD` and
  `RESET_ADMIN_PASSWORD_ON_START=true` for one successful start, then set the flag back to false.
  The reset also invalidates every JWT previously issued to that account.

Run one backend replica per configuration database. The backend combines a PostgreSQL advisory
lock with a renewable 30-second database lease. It refuses a second replica and shuts down
fail-closed if the lock session is lost, so crash recovery cannot mistake another worker's
in-progress backup for an abandoned run. After an unclean database/backend restart, a replacement
may wait up to 30 seconds for the old lease to expire.

The dashboard is served by Nginx on `http://localhost` by default. The backend is available inside Docker Compose as `backend:3000` and is exposed through `/api`.

The backend exposes unauthenticated probes for container orchestrators and external monitors:

- `GET /api/health/live` confirms that the process is serving requests.
- `GET /api/health/ready` verifies both the configuration database and a real create/write/delete
  cycle in the backup directory. It returns HTTP `503` when either dependency is unavailable.
- `GET /api/health` is a backwards-compatible alias for the readiness probe.

Completed backup files are written inside the backend container at `/backups`. Set `BACKUP_STORAGE_PATH` in `.env` to choose where that directory is mounted on the host:

```bash
BACKUP_STORAGE_PATH=/mnt/storage/dumply/backups
```

Relative paths such as `./backups` are resolved from the directory containing `docker-compose.yml`.

For production monitoring, cancellation behavior, restore drills, incident response, and the
deployment checklist, see [docs/OPERATIONS.md](docs/OPERATIONS.md). A backup is not considered
proven until it has been restored and checked in an isolated environment.

Email notifications use the SMTP sender configured in `.env`. You can edit `SMTP_HOST`, `SMTP_PORT`, `SMTP_USERNAME`, `SMTP_PASSWORD`, `SMTP_FROM`, and `SMTP_TLS` manually, or run:

```bash
sh scripts/configure-email.sh
```

The helper stores SMTP values in Compose-compatible literal quotes, so common password characters
such as `$`, `#`, and embedded spaces round-trip unchanged. It rejects single quotes and
backslashes because Compose and the host dotenv parser do not interpret every escaped form
identically. Verify the local loader and, when Docker is available, Compose itself with
`bash scripts/test-env-roundtrip.sh`.

For TLS, place `fullchain.pem` and `privkey.pem` under `./certs`, set `ENABLE_TLS=true`, then include
the TLS port override:

```bash
docker compose -f docker-compose.yml -f docker-compose.tls.yml up --build
```

The default non-TLS stack does not reserve the host's HTTPS port.

The bundled Nginx rate-limits login requests by its direct client IP. If another load balancer or
reverse proxy sits in front, configure trusted real-IP handling and login limiting at that outer
layer; do not trust arbitrary `X-Forwarded-For` headers.

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
Docker dev backup files are stored under `DEV_BACKUP_STORAGE_PATH` on the host and mounted to `/backups` in the backend container.
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
- Bootstrap and explicit one-start admin password reset from startup configuration.
- JWT login/logout and active-user authorization; logout invalidates all sessions for that user,
  and deleted users lose access immediately.
- User list/create/delete for admin users.
- Backup config list/create/update/delete/toggle with encrypted `db_url` storage and masked API output.
- Manual trigger endpoint that starts a guarded background backup worker.
- Asynchronous backup cancellation that terminates the dump process tree, removes partial output,
  records a `cancelled` terminal state, and exposes progress in the Run Room.
- PostgreSQL backup via version-matched `pg_dump` using `PGPASSWORD` instead of password arguments.
- MySQL backup via system `mysqldump` using a temporary option file instead of password arguments.
- Backup timeout handling, private atomic output files, restart recovery, and startup/hourly
  retention sweeps. Retention re-reads and locks the current policy, then queues file removal in
  the same durable cleanup outbox used by config deletion.
- Cron scheduler using 6-field schedules (`sec min hour day month weekday`) for enabled configs.
- Backup download streaming with an authenticated availability preflight and canonical
  `BACKUP_DIR` path checks.
- Config deletion serializes against triggers, commits a durable file-deletion intent atomically,
  and retries managed-file cleanup without risking a database rollback that points at a lost file.
- Readiness and liveness endpoints, with Docker health gating before Nginx starts.
- Dashboard stats for backup coverage, active runs, last success, outcomes, storage, and cleanup
  backlog; config rows also expose their latest run and last successful run.
- React dashboard for login, operational stats, backup config CRUD, manual trigger/cancel, Run Room
  events, history download, and user management.
- Nginx reverse proxy serving the frontend and proxying `/api` to the backend.
- Optional TLS mode controlled by `ENABLE_TLS`.
- Backend graceful shutdown and configurable CORS origin.

## Pending

- Automated integration tests against live PostgreSQL/MySQL targets.
- Automated restore verification and off-host storage replication.
