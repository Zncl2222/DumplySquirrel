# DumplySquirrel Operations Guide

This guide defines the minimum production practices around DumplySquirrel. The application can
produce and retain database dumps; availability and recoverability still depend on how the host,
backup mount, monitoring, and restore drills are operated.

## Production launch checklist

- Generate unique `DB_PASSWORD`, `JWT_SECRET`, `DATABASE_ENCRYPTION_KEY`, and `ADMIN_PASSWORD`
  values with `./scripts/init-env.sh`; do not deploy any `change_me_*` value.
- Mount `BACKUP_STORAGE_PATH` on durable storage with enough capacity for the retention policy.
- Make that directory writable by the backend container. The production service drops all Linux
  capabilities, so it deliberately cannot bypass host ownership or mode bits on a bind mount.
- Size `MAX_BACKUP_FILE_BYTES` and `MIN_FREE_DISK_BYTES` for that filesystem. The effective limit
  of each run is the smaller of its configured cap and its safe share of currently available
  space, so concurrent dumps cannot intentionally consume the reserved free space.
- Replicate or snapshot that storage off-host. A bind mount on the database host is not an
  independent backup when that host is lost.
- Terminate TLS either with the included TLS mode or a trusted upstream proxy. Restrict dashboard
  access to the intended operators.
- Keep the default loopback-only HTTP bind unless it is used solely for a public HTTPS redirect.
  Do not transmit dashboard passwords or bearer sessions over public plain HTTP.
- Require encrypted target-database connections. For PostgreSQL, use `sslmode=verify-full` (and
  the appropriate `sslrootcert`) in the target URL. For MySQL, use `ssl-mode=verify-identity` with
  an absolute `ssl-ca` path mounted inside the backend container; also require TLS on the database
  account so a client-side configuration mistake fails closed.
- Configure SMTP and test at least one successful and one failed notification path when email is
  part of the incident workflow.
- Send container logs to persistent log storage and configure the health/backup alerts below.
- Preserve the production Compose security settings when adapting deployment manifests: read-only
  backend/Nginx roots, bounded PID counts, no-new-privileges, and restricted capabilities.
- Complete a restore drill before relying on the system, and repeat it on a fixed schedule.

## Health and monitoring

| Probe or signal | Meaning | Recommended action |
|---|---|---|
| `GET /api/health/live` | Backend process can answer HTTP | Restart or investigate when repeatedly unavailable |
| `GET /api/health/ready` | Config DB is queryable and backup storage accepts a real write/delete probe | Remove the instance from traffic and page an operator after two consecutive failures |
| Scheduled run is `failed` or `timeout` | No usable output was committed | Investigate immediately; retry only after the cause is understood |
| Task has no available successful backup | The dashboard's protected-task numerator excludes it | Run and verify a backup before treating the task as protected |
| No success within twice the task cadence | Recovery point objective is at risk | Check scheduler, target connectivity, credentials, and capacity |
| Pending cleanup remains non-zero for one hour | Filesystem deletion is failing or the mount is unavailable | Inspect backend logs and storage permissions/capacity |

The readiness probe deliberately creates and removes a randomized hidden file. A read-only or full
mount therefore fails readiness even when directory metadata can still be listed.

## Target database transport

PostgreSQL connection parameters after `?` are passed to libpq through a password-free connection
URI, while the password remains in `PGPASSWORD`. Identity and credential overrides such as
`host`, `user`, `password`, `passfile`, and `service` are rejected so the audited target cannot be
silently changed by a second parameter source. PostgreSQL certificate-path parameters are also
restricted to traversal-free paths below the read-only `/etc/dumply-certs` mount. Example:

```text
postgres://backup_user:password@db.example.com/app?sslmode=verify-full&sslrootcert=/etc/dumply-certs/postgres-ca.pem
```

MySQL URLs accept only `ssl-mode`, `ssl-ca`, `ssl-capath`, `ssl-cert`, `ssl-key`, and
`tls-version`. Supported modes are `disabled`, `required`, `verify-ca`, and `verify-identity`;
`verify-ca` is deliberately strengthened to host-name verification by the bundled client. CA,
certificate, and key paths must be traversal-free paths below `/etc/dumply-certs`. Put their host
files under `DB_CERT_STORAGE_PATH`; Compose mounts that directory read-only. Example:

```text
mysql://backup_user:password@db.example.com/app?ssl-mode=verify-identity&ssl-ca=/etc/dumply-certs/mysql-ca.pem
```

Never place certificate-key passwords in URL query parameters. Independently configure the target
database account to require TLS.

## Cancelling a backup

Use **Cancel backup** in the Run Room. The API accepts the request asynchronously with HTTP `202`,
terminates the dump process group, removes partial output, and records a `cancelled` event and
terminal history status. Keep the Run Room open until the terminal status appears.

Cancellation is a best-effort race at the very end of a run: if the dump has already been sealed
and committed when the request arrives, the run can correctly finish as `success`. A cancelled run
never exposes its partial file as downloadable.

## Restore drill

Run drills only against an isolated, disposable database. Download a recent backup from the
history screen and use the target database's normal secure credential mechanism; do not paste
production passwords into shell history.

PostgreSQL plain-text dumps can be restored with:

```bash
createdb dumply_restore_drill
psql --set ON_ERROR_STOP=1 --dbname dumply_restore_drill --file backup.sql
```

MySQL dumps can be restored with:

```bash
mysql --database dumply_restore_drill < backup.sql
```

A successful import is necessary but not sufficient. Record the backup history ID, drill date,
duration, database engine/version, operator, and results of application-specific checks such as
row counts, required tables, recent transactions, and a read-only application smoke test. Delete
the isolated restored database after evidence has been retained.

## Incident triage

1. Check `/api/health/ready` and the affected Run Room event sequence.
2. Confirm free space, mount availability, and write permissions on `BACKUP_STORAGE_PATH`.
3. Confirm the target database is reachable from the backend container and that credentials have
   not expired.
4. For PostgreSQL, verify that a client matching the supported server major version (14–18) is
   available. For MySQL, verify compatibility with the bundled system client.
5. Preserve logs and the history ID before retrying. Repeated retries can hide a persistent fault
   and consume execution slots.
6. If the configuration database or backend restarted mid-run, allow startup recovery to reconcile
   the history row and any atomically sealed output before taking manual filesystem action.

Never manually rename a `.part` file into a backup. Only files sealed and committed by the worker
are downloadable and counted as available protection.
