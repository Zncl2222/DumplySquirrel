# Dev Container

A single VS Code dev container carrying both toolchains for this full-stack repo:

- **Rust 1.x** (backend — Axum + sqlx) with `rustfmt`, `clippy`, and the
  Postgres (14–18) and MySQL client tools that backup jobs shell out to.
- **Node 20** (frontend — Vite + React + TypeScript).

It reuses the existing [`docker-compose.dev.yml`](../docker-compose.dev.yml) so
**Postgres runs alongside** the workspace on the same network.

## Usage

1. Copy `.env.example` to `.env` and fill in the required secrets (`DB_PASSWORD`,
   `JWT_SECRET`, `DATABASE_ENCRYPTION_KEY`, `ADMIN_PASSWORD`). The container reads
   these at start, same as the normal dev stack.
2. In VS Code: **Dev Containers: Reopen in Container** (needs the *Dev Containers*
   extension + Docker).
3. Once inside, run each service from the integrated terminal:

   ```bash
   cd backend && cargo run        # http://localhost:8000  (runs migrations on start)
   cd frontend && npm run dev      # http://localhost:5173
   ```

Postgres is reachable at `postgres:5432` from inside the container and forwarded
to `localhost:5432` on the host.

## What starts automatically

Only `postgres` and the `workspace` container start (`runServices` in
`devcontainer.json`). The `backend` and `frontend` compose services are left off
so you run them yourself without port conflicts — edit `runServices` if you'd
rather have them auto-start.
