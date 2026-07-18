# DumplySquirrel — 備份管理系統規格書

> 最後更新：2026-06-12

---

## 目錄

1. [專案概述](#1-專案概述)
2. [系統架構](#2-系統架構)
3. [技術棧](#3-技術棧)
4. [資料庫 Schema](#4-資料庫-schema)
5. [API 設計](#5-api-設計)
6. [後端模組設計](#6-後端模組設計)
7. [前端頁面規劃](#7-前端頁面規劃)
8. [Docker 與部署](#8-docker-與部署)
9. [Nginx Reverse Proxy 與 TLS](#9-nginx-reverse-proxy-與-tls)
10. [專案目錄結構](#10-專案目錄結構)
11. [實作補強與風險控制](#11-實作補強與風險控制)
12. [開發順序建議](#12-開發順序建議)

---

## 1. 專案概述

Dockerized 備份管理系統，提供 Web Dashboard 讓管理者設定、排程、觸發與檢視 PostgreSQL / MySQL 資料庫的備份任務。備份檔案存放於本機磁碟掛載（可作為異地備援使用）。

| 項目 | 內容 |
|------|------|
| 專案名稱 | DumplySquirrel |
| 後端語言 | Rust |
| 前端框架 | React (Vite) |
| 設定資料庫 | PostgreSQL |
| 備份對象 | PostgreSQL (pg_dump)、MySQL (mysqldump) |
| 備份儲存 | Docker volume / bind mount 掛載至本機 |
| 部署方式 | Docker Compose |
| 還原方式 | 使用者自行手動操作（Dashboard 僅提供檔案下載） |

---

## 2. 系統架構

```
                         ┌──────────────┐
                    :443 │   Nginx      │
  Client ───────────────▶│  (TLS 終止)   │
                    :80  │  Reverse     │
                         │  Proxy       │
                         └──┬───────┬───┘
                            │       │
                     /api/* │       │ 靜態檔案
                            ▼       ▼
                      ┌────────┐ ┌──────────┐
                      │  Rust  │ │  React   │
                      │ Backend│ │  (built) │
                      │ (Axum) │ └──────────┘
                      └───┬────┘
                          │
                    ┌─────▼──────┐
                    │ PostgreSQL │  — 管理設定、帳號
                    │  (config)  │
                    └─────┬──────┘
                          │
                ┌─────────▼──────────┐
                │  pg_dump /         │
                │  mysqldump         │  — 執行備份
                │  + Scheduler       │
                └─────────┬──────────┘
                          │
                    ┌─────▼──────┐
                    │   Backup   │
                    │   Volume   │  — 存放 .sql 備份檔
                    └────────────┘
```

---

## 3. 技術棧

### 後端 (Rust)

| 套件 | 用途 |
|------|------|
| `axum` | Web Framework |
| `tokio` (full) | Async runtime |
| `sqlx` (postgres) | 資料庫連線（compile-time checked queries） |
| `serde` / `serde_json` | 序列化 |
| `jsonwebtoken` | JWT 驗證 |
| `bcrypt` | 密碼雜湊 |
| `tokio-cron-scheduler` | 排程任務 |
| `tower-http` | CORS、tracing 中介層 |
| `chrono` | 時間處理 |
| `uuid` | ID 產生 |
| `tracing` / `tracing-subscriber` | 結構化日誌 |
| `dotenvy` | .env 載入 |

### 前端 (React)

| 工具 | 用途 |
|------|------|
| Vite | 建置工具 |
| React Router v7 | 前端路由 |
| shadcn/ui + Tailwind CSS | UI 元件庫（或 MUI 簡潔版） |
| React Query (TanStack Query) | API 狀態管理與快取 |
| axios | HTTP client |

**備份執行容器內需預裝**：`postgresql-client`、`mysql-client`（於 Dockerfile 中 apt install）。

---

## 4. 資料庫 Schema

### 4.1 `users` — 管理員帳號

| 欄位 | 型態 | 約束 | 說明 |
|------|------|------|------|
| `id` | UUID | PK | |
| `username` | VARCHAR(100) | UNIQUE, NOT NULL | 登入帳號 |
| `password_hash` | VARCHAR(255) | NOT NULL | bcrypt 雜湊 |
| `role` | VARCHAR(20) | NOT NULL, DEFAULT 'admin' | 角色 |
| `token_version` | BIGINT | NOT NULL, DEFAULT 0 | 登出或密碼重設時遞增，使舊 JWT 失效 |
| `created_at` | TIMESTAMPTZ | NOT NULL, DEFAULT now() | |
| `updated_at` | TIMESTAMPTZ | NOT NULL, DEFAULT now() | |

### 4.2 `backup_configs` — 備份任務設定

| 欄位 | 型態 | 約束 | 說明 |
|------|------|------|------|
| `id` | UUID | PK | |
| `name` | VARCHAR(200) | NOT NULL | 任務名稱 |
| `db_type` | VARCHAR(20) | NOT NULL | `postgres` 或 `mysql`（預留擴充） |
| `db_url_encrypted` | TEXT | NOT NULL | 加密後的目標資料庫連線字串 |
| `db_url_nonce` | TEXT | NOT NULL | 加密 nonce/base64 |
| `cron_schedule` | VARCHAR(100) | NULLABLE | cron 表達式，null 表示僅手動 |
| `is_enabled` | BOOLEAN | NOT NULL, DEFAULT true | 啟用/停用 |
| `retention_days` | INTEGER | NOT NULL, DEFAULT 30 | 備份保留天數 |
| `timeout_seconds` | INTEGER | NOT NULL, DEFAULT 3600 | 單次備份逾時秒數 |
| `max_backups` | INTEGER | NULLABLE | 每個任務最多保留份數，null 表示不限 |
| `created_by` | UUID | FK → users(id) | 建立者 |
| `created_at` | TIMESTAMPTZ | NOT NULL, DEFAULT now() | |
| `updated_at` | TIMESTAMPTZ | NOT NULL, DEFAULT now() | |

### 4.3 `backup_history` — 備份執行記錄

| 欄位 | 型態 | 約束 | 說明 |
|------|------|------|------|
| `id` | UUID | PK | |
| `config_id` | UUID | FK → backup_configs(id) ON DELETE CASCADE | |
| `status` | VARCHAR(20) | NOT NULL | `running` / `success` / `failed` |
| `file_name` | VARCHAR(255) | NULLABLE | 備份檔名 |
| `file_size` | BIGINT | NULLABLE | 檔案大小（bytes） |
| `file_path` | TEXT | NULLABLE | 容器內路徑 |
| `error_message` | TEXT | NULLABLE | 錯誤訊息 |
| `started_at` | TIMESTAMPTZ | NOT NULL | 開始時間 |
| `completed_at` | TIMESTAMPTZ | NULLABLE | 完成時間 |
| `triggered_by` | VARCHAR(20) | NOT NULL | `manual` 或 `scheduled` |

> `backup_history.status` 可用值：`running` / `success` / `failed` / `timeout` / `cancelled`。

---

## 5. API 設計

### 5.1 Auth

| Method | Path | Body | 說明 |
|--------|------|------|------|
| `POST` | `/api/auth/login` | `{ username, password }` | 登入，回傳 JWT |
| `POST` | `/api/auth/logout` | — | 登出（清除 token） |
| `GET` | `/api/auth/me` | — | 當前使用者資訊（需 JWT） |

### 5.2 Users（需 admin 權限 + JWT）

| Method | Path | Body / Param | 說明 |
|--------|------|------|------|
| `GET` | `/api/users` | — | 使用者列表 |
| `POST` | `/api/users` | `{ username, password, role? }` | 新增使用者 |
| `DELETE` | `/api/users/:id` | — | 刪除使用者 |

### 5.3 Backup Configs（需 JWT）

| Method | Path | Body / Param | 說明 |
|--------|------|------|------|
| `GET` | `/api/backup-configs` | — | 列表（含啟用狀態、最近一次備份時間） |
| `POST` | `/api/backup-configs` | `{ name, db_type, db_url, cron_schedule?, retention_days?, timeout_seconds?, max_backups? }` | 新增設定；`db_url` 只接受寫入，不完整回傳 |
| `PUT` | `/api/backup-configs/:id` | 同上（部分更新） | 編輯設定 |
| `DELETE` | `/api/backup-configs/:id` | — | 刪除設定 |
| `POST` | `/api/backup-configs/:id/trigger` | — | 手動立即備份 |
| `PATCH` | `/api/backup-configs/:id/toggle` | `{ is_enabled }` | 啟用/停用 |

### 5.4 Backup History（需 JWT）

| Method | Path | Query / Param | 說明 |
|--------|------|------|------|
| `GET` | `/api/backup-history` | `?config_id=&status=&page=&per_page=` | 列表（可過濾、分頁） |
| `GET` | `/api/backup-history/:id` | — | 單筆記錄明細 |
| `GET` | `/api/backup-history/:id/download` | — | 下載備份檔案（stream） |

### 5.5 Dashboard Stats（需 JWT）

| Method | Path | 說明 |
|--------|------|------|
| `GET` | `/api/dashboard/stats` | 總任務數、總備份次數、成功/失敗數、本日備份數、儲存總用量 |

### 5.6 Response 格式

```json
// 成功
{ "data": { ... } }

// 列表含分頁
{ "data": [...], "pagination": { "page": 1, "per_page": 20, "total": 100 } }

// 錯誤
{ "error": { "code": "NOT_FOUND", "message": "..." } }
```

---

## 6. 後端模組設計

```
backend/src/
├── main.rs              # 啟動 server、初始化 DB、排程器
├── config.rs            # 環境變數讀取（DatabaseUrl, JwtSecret, BackupDir...）
├── error.rs             # 統一錯誤處理
│
├── db/
│   ├── mod.rs           # sqlx pool 初始化
│   └── models.rs        # DB model structs（對應 schema）
│
├── routes/
│   ├── mod.rs           # 註冊所有路由
│   ├── auth.rs          # login, logout, me
│   ├── backups.rs       # backup config CRUD + trigger + toggle
│   ├── history.rs       # backup history list + detail + download
│   ├── users.rs         # user CRUD
│   └── dashboard.rs     # dashboard stats
│
├── services/
│   ├── mod.rs
│   ├── backup_executor.rs   # 核心：讀取 db_url → 執行 pg_dump/mysqldump
│   └── scheduler.rs         # tokio-cron-scheduler 管理
│
└── middleware/
    └── auth.rs              # JWT 驗證中介層
```

### 6.1 `backup_executor.rs` 核心邏輯

```rust
pub async fn execute_backup(config: BackupConfig, backup_dir: &Path) -> Result<BackupRecord> {
    let file_name = format!("{}_{}_{}.sql", config.id, chrono::Utc::now().format("%Y%m%d_%H%M%S"), history.id);
    let output_path = backup_dir.join(&file_name);

    let status = match config.db_type.as_str() {
        "postgres" => run_pg_dump(&config.db_url, &output_path).await?,
        "mysql"    => run_mysqldump(&config.db_url, &output_path).await?,
        _          => return Err(unsupported error),
    };

    // 寫入 backup_history
    // 以目前最新 retention policy 將過期檔案原子寫入 durable deletion outbox
}
```

- 使用 `tokio::process::Command` 執行子行程
- 串流 stdout 直接寫入檔案，避免記憶體爆量
- PostgreSQL 使用 `PGPASSWORD` 環境變數傳遞密碼（pg_dump），不可將密碼寫入命令列參數
- MySQL 使用暫存 option file 或安全 stdin/config file 傳遞密碼，不可使用 `-p'password'` 命令列參數
- 檔名使用 `config.id + timestamp + history.id` 產生，任務名稱僅作顯示用途，避免 path traversal、同秒碰撞與非法檔名
- dump 先寫入權限 `0600` 的唯一 partial file，完成後需 fsync 檔案、原子 rename 並 fsync 父目錄；持久化未確認前不可標記 success
- 每個 config 同時間只允許一個備份執行；全域備份 worker 需限制最大併發數
- 單次備份需套用 `timeout_seconds`，逾時後終止子行程並標記為 `timeout`
- 下載備份檔時必須確認 canonical path 位於 `BACKUP_DIR` 內，不可直接信任資料庫中的 `file_path`

---

## 7. 前端頁面規劃

### 7.1 路由

| 路徑 | 頁面 | 說明 |
|------|------|------|
| `/login` | LoginPage | 登入表單 |
| `/` | DashboardPage | 總覽統計 |
| `/backup-configs` | ConfigListPage | 備份設定列表（CRUD） |
| `/backup-configs/new` | ConfigFormPage | 新增設定 |
| `/backup-configs/:id/edit` | ConfigFormPage | 編輯設定 |
| `/history` | HistoryPage | 備份歷史記錄（篩選、分頁、下載） |
| `/users` | UsersPage | 使用者管理（僅 admin） |

### 7.2 共用元件

- `Layout` — 側邊欄 + Header，含導航
- `AuthGuard` — 未登入時導向 /login
- `DataTable` — 表格（排序、篩選、分頁）
- `ConfirmDialog` — 刪除確認對話框
- `StatusBadge` — 成功/失敗/執行中徽章
- `CronInput` — cron 表達式輔助輸入

---

## 8. Docker 與部署

### 8.1 `docker-compose.yml` 三服務

```yaml
services:
  postgres:
    image: postgres:16-alpine
    restart: unless-stopped
    volumes:
      - pgdata:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U dumply -d dumply_squirrel"]
      interval: 10s
      timeout: 5s
      retries: 5
    environment:
      POSTGRES_DB: dumply_squirrel
      POSTGRES_USER: dumply
      POSTGRES_PASSWORD: ${DB_PASSWORD:?required}

  backend:
    build:
      context: ./backend
      dockerfile: Dockerfile
    restart: unless-stopped
    depends_on:
      postgres:
        condition: service_healthy
    environment:
      DATABASE_URL: postgres://dumply:${DB_PASSWORD_URL_ENCODED:-${DB_PASSWORD:?required}}@postgres:5432/dumply_squirrel
      JWT_SECRET: ${JWT_SECRET:?required}
      DATABASE_ENCRYPTION_KEY: ${DATABASE_ENCRYPTION_KEY:?required}
      ADMIN_USERNAME: ${ADMIN_USERNAME:-admin}
      ADMIN_PASSWORD: ${ADMIN_PASSWORD:?required}
      BACKUP_DIR: /backups
      RUST_LOG: info
    volumes:
      - ${BACKUP_STORAGE_PATH:-./backups}:/backups

  frontend:
    build:
      context: ./frontend
      dockerfile: Dockerfile
    restart: unless-stopped
    ports:
      - "${HTTP_PORT:-80}:${HTTP_PORT:-80}"
    depends_on: [backend]
    environment:
      NGINX_PORT: ${HTTP_PORT:-80}
      NGINX_HTTPS_PORT: ${HTTPS_PORT:-443}
      SERVER_NAME: ${SERVER_NAME:-_}
      ENABLE_TLS: ${ENABLE_TLS:-false}
      TLS_CERT_FILE: /etc/nginx/certs/fullchain.pem
      TLS_KEY_FILE: /etc/nginx/certs/privkey.pem
    volumes:
      - ${TLS_CERT_DIR:-./certs}:/etc/nginx/certs:ro

volumes:
  pgdata:
```

### 8.2 Backend Dockerfile（multi-stage）

```dockerfile
# Stage 1: Build
FROM rust:1.88-slim-bookworm AS builder
WORKDIR /app
RUN apt-get update && apt-get install -y pkg-config libssl-dev
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs
RUN cargo build --release 2>/dev/null || true  # cache deps
COPY . .
RUN cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    postgresql-client \
    default-mysql-client \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/dumply-backend /usr/local/bin/
EXPOSE 3000
CMD ["dumply-backend"]
```

### 8.3 環境變數 `.env.example`

```bash
# Use scripts/init-env.sh to generate unique values; public placeholders are rejected.
DB_PASSWORD=<generated-random-value>
DB_PASSWORD_URL_ENCODED=

# JWT
JWT_SECRET=<generated-random-value>

# Field encryption for target database URLs
# The initializer generates 32 random bytes encoded as hexadecimal.
DATABASE_ENCRYPTION_KEY=<generated-random-value>
DATABASE_ENCRYPTION_KEY_PREVIOUS=

# Bootstrap admin
ADMIN_USERNAME=admin
ADMIN_PASSWORD=<generated-random-value>
RESET_ADMIN_PASSWORD_ON_START=false

# Server
SERVER_NAME=backup.example.com
HTTP_PORT=80
HTTPS_PORT=443
BACKUP_STORAGE_PATH=./backups

# TLS (optional)
ENABLE_TLS=false
TLS_CERT_DIR=./certs
```

---

## 9. Nginx Reverse Proxy 與 TLS

### 9.1 架構說明

- Nginx 作為統一入口，對外暴露 80/443
- TLS 終止於 Nginx 層
- `/api/*` → proxy_pass 到 backend:3000（內部網路，無需 CORS）
- 其餘路徑 → serve React 靜態檔案
- 一套配置同時支援 TLS 啟用/關閉，透過 `ENABLE_TLS` 環境變數控制

### 9.2 Nginx 配置模板

檔案位置：`frontend/nginx/templates/default.conf.template`

```nginx
server {
    listen ${NGINX_PORT:-80};
    server_name ${SERVER_NAME:-_};

    # API 反向代理
    location /api/ {
        proxy_pass http://backend:3000;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # 較大 body 以支援大型備份設定
        client_max_body_size 10m;
    }

    # React 靜態檔案
    location / {
        root /usr/share/nginx/html;
        index index.html;
        try_files $uri $uri/ /index.html;

        # 靜態檔案快取
        location ~* \.(js|css|png|jpg|jpeg|gif|ico|svg)$ {
            expires 1y;
            add_header Cache-Control "public, immutable";
        }
    }
}
```

### 9.3 TLS 配置區塊（條件式引入）

當 `ENABLE_TLS=true` 時，Nginx entrypoint script 會插入以下區塊至配置檔：

```
listen ${NGINX_HTTPS_PORT:-443} ssl;
ssl_certificate     ${TLS_CERT_FILE:-/etc/nginx/certs/fullchain.pem};
ssl_certificate_key ${TLS_KEY_FILE:-/etc/nginx/certs/privkey.pem};
ssl_protocols       TLSv1.2 TLSv1.3;
ssl_ciphers         HIGH:!aNULL:!MD5;

# 將 HTTP 導向 HTTPS（可選）
# return 308 https://$host:${NGINX_HTTPS_PORT}$request_uri;
```

> 實現方式：使用 nginx:alpine 內建的 `/docker-entrypoint.d/` 機制，或撰寫自訂 entrypoint script 根據 `$ENABLE_TLS` 動態產生完整配置。

### 9.4 Frontend Dockerfile

```dockerfile
# Stage 1: Build React
FROM node:24-alpine AS builder
WORKDIR /app
COPY package*.json ./
RUN npm ci
COPY . .
ARG VITE_API_BASE_URL=/api
RUN npm run build

# Stage 2: Serve with nginx
FROM nginx:1.30.4-alpine
COPY --from=builder /app/dist /usr/share/nginx/html
COPY nginx/templates /etc/nginx/custom-templates
COPY nginx/docker-entrypoint.d /docker-entrypoint.d
RUN chmod +x /docker-entrypoint.d/99-dumply-render-config.sh
# 自訂 entrypoint hook 只渲染 ENABLE_TLS 對應的單一 template。
```

### 9.5 TLS 使用方式

**開發環境（無 TLS）：**
```bash
docker compose up
```

**正式環境（有 TLS）：**
```bash
# 先將憑證放置於 ./certs/
# ├── certs/
# │   ├── fullchain.pem
# │   └── privkey.pem

ENABLE_TLS=true docker compose -f docker-compose.yml -f docker-compose.tls.yml up
```

| 環境變數 | 預設值 | 說明 |
|---------|--------|------|
| `ENABLE_TLS` | `false` | 啟用 HTTPS |
| `NGINX_PORT` | `80` | HTTP 埠 |
| `NGINX_HTTPS_PORT` | `443` | HTTPS 埠（ENABLE_TLS=true 時生效） |
| `SERVER_NAME` | `_` | 伺服器域名 |
| `TLS_CERT_FILE` | `/etc/nginx/certs/fullchain.pem` | 憑證路徑 |
| `TLS_KEY_FILE` | `/etc/nginx/certs/privkey.pem` | 私鑰路徑 |

---

## 10. 專案目錄結構

```
DumplySquirrel/
│
├── backend/                          # Rust Axum 後端
│   ├── src/
│   │   ├── main.rs                   # 應用入口
│   │   ├── config.rs                 # 環境變數配置
│   │   ├── error.rs                  # 統一錯誤處理
│   │   ├── db/
│   │   │   ├── mod.rs                # DB pool 初始化
│   │   │   └── models.rs             # 資料表 struct
│   │   ├── routes/
│   │   │   ├── mod.rs                # 路由註冊
│   │   │   ├── auth.rs               # 認證路由
│   │   │   ├── backups.rs            # 備份設定路由
│   │   │   ├── history.rs            # 備份歷史路由
│   │   │   ├── users.rs              # 使用者路由
│   │   │   └── dashboard.rs          # Dashboard 統計路由
│   │   ├── services/
│   │   │   ├── mod.rs
│   │   │   ├── backup_executor.rs    # pg_dump / mysqldump 執行
│   │   │   └── scheduler.rs          # 排程任務管理
│   │   └── middleware/
│   │       └── auth.rs               # JWT 驗證中介層
│   ├── migrations/                   # sqlx migrations
│   │   ├── 001_create_users.sql
│   │   ├── 002_create_backup_configs.sql
│   │   └── 003_create_backup_history.sql
│   ├── Cargo.toml
│   └── Dockerfile
│
├── frontend/                         # React Vite 前端
│   ├── src/
│   │   ├── api/                      # API client（axios）
│   │   │   ├── client.ts
│   │   │   ├── auth.ts
│   │   │   ├── backups.ts
│   │   │   ├── history.ts
│   │   │   └── users.ts
│   │   ├── components/               # 共用元件
│   │   │   ├── Layout.tsx
│   │   │   ├── AuthGuard.tsx
│   │   │   ├── DataTable.tsx
│   │   │   ├── ConfirmDialog.tsx
│   │   │   ├── StatusBadge.tsx
│   │   │   └── CronInput.tsx
│   │   ├── pages/                    # 頁面
│   │   │   ├── LoginPage.tsx
│   │   │   ├── DashboardPage.tsx
│   │   │   ├── ConfigListPage.tsx
│   │   │   ├── ConfigFormPage.tsx
│   │   │   ├── HistoryPage.tsx
│   │   │   └── UsersPage.tsx
│   │   ├── hooks/                    # 自訂 hooks
│   │   ├── lib/                      # 工具函式
│   │   ├── App.tsx
│   │   ├── main.tsx
│   │   └── index.css
│   ├── nginx/
│   │   └── templates/
│   │       └── default.conf.template # Nginx 配置模板
│   ├── Dockerfile
│   ├── vite.config.ts
│   ├── tsconfig.json
│   └── package.json
│
├── docker-compose.yml
├── .env.example
├── .gitignore
├── SPECIFICATION.md                  # 本規格書
└── README.md
```

---

## 11. 實作補強與風險控制

### 11.1 敏感資料與認證

- `db_url` 不得以明文存入資料庫；後端以 `DATABASE_ENCRYPTION_KEY` 加密後存成 `db_url_encrypted` 與 `db_url_nonce`。
- API 回傳 backup config 時，一律回傳遮蔽後的 `db_url_masked`，例如 `postgres://user:****@host/db`。
- JWT 必須設定短期效期，建議 1-8 小時；受保護 API 必須比對使用者的 `token_version`。
  `logout` 與管理員密碼重設會遞增版本，使該使用者先前簽發的 JWT 持久失效。
- 密碼使用 bcrypt；新增/修改使用者時需做最小長度驗證。
- 首次部署透過 `ADMIN_USERNAME` / `ADMIN_PASSWORD` 建立 bootstrap admin，若已存在任何 user 則不重複建立。

### 11.2 備份執行安全

- PostgreSQL / MySQL DSN 需明確解析，並安全傳遞帳密給 dump 工具；不得把密碼放在命令列參數。
- 備份檔名由 `config_id + UTC timestamp` 組成，不使用使用者輸入的任務名稱。
- 同一個 config 同時間只允許一個 running backup；全域需限制最大併發數，避免壓垮目標資料庫。
- 每次備份有 timeout；逾時需 kill child process、標記 `timeout`，並清理不完整檔案。
- retention 清理同時依 `retention_days` 與 `max_backups` 執行；啟動時及每小時重跑。
  清理必須鎖定並重讀最新設定，再把檔案原子寫入 durable deletion outbox；刪除失敗需保留重試狀態與錯誤日誌。

### 11.3 排程器規則

- cron 格式採 `tokio-cron-scheduler` 的 6 欄位格式：`sec min hour day month weekday`。
- 所有排程以 UTC 執行；若未來要支援時區，需在 schema 增加 `timezone`。
- 建立、更新、停用、刪除 config 後，scheduler 必須重新註冊對應 job。
- 服務啟動時載入所有 `is_enabled = true` 且有 `cron_schedule` 的 config。

### 11.4 下載與檔案系統防護

- 下載 API 只能使用 history id 查詢系統產生的檔案，不接受任意檔案路徑參數。
- 前端以帶 Authorization 的 HEAD 先確認 session 與檔案可用，再用 native POST 讓瀏覽器直接串流，JWT 不得放入 URL。
- 讀檔前需 canonicalize 並確認實際路徑仍位於 `BACKUP_DIR` 下。
- 若 history 存在但檔案已被 retention 或人工刪除，應回傳 `404 FILE_MISSING`。

### 11.5 部署可行性

- Compose 需為 PostgreSQL 定義 healthcheck，backend 依健康狀態啟動。
- backend runtime image 使用 Debian 可用的 `postgresql-client` 與 `default-mysql-client` 套件名。
- Nginx `proxy_pass http://backend:3000;` 保留 `/api/*` path，與後端路由一致。
- TLS 條件式配置需用自訂 entrypoint 或兩份 template 實作，不能只依賴 nginx envsubst 自動插入邏輯區塊。

---

## 12. 開發順序建議

| 階段 | 內容 | 產出 |
|------|------|------|
| 1 | Rust 專案初始化，Axum server 骨架，sqlx 連線 DB | 可啟動的 backend |
| 2 | Database migrations（users, backup_configs, backup_history） | DB schema |
| 3 | JWT 認證 middleware + auth routes | 可登入/登出 |
| 4 | User CRUD API | 使用者管理 |
| 5 | Backup config CRUD API | 備份設定管理 |
| 6 | backup_executor 服務（pg_dump / mysqldump） | 可執行備份 |
| 7 | Backup history API + 檔案下載 | 歷史記錄與下載 |
| 8 | Scheduler 服務（cron 排程自動備份） | 自動備份 |
| 9 | Dashboard 統計 API | 儀表板資料 |
| 10 | React 前端：Login + Dashboard 頁面 | 前端雛形 |
| 11 | React 前端：Backup Config CRUD 頁面 | 設定管理 UI |
| 12 | React 前端：History + Download 頁面 | 歷史紀錄 UI |
| 13 | React 前端：User Management 頁面 | 使用者 UI |
| 14 | Dockerfile（backend + frontend） | 容器化 |
| 15 | docker-compose.yml + Nginx 配置 | 一鍵啟動 |
| 16 | README + .env.example | 文件 |

---

*本規格書對應 DumplySquirrel 專案，後端 Rust Axum + 前端 React Vite + PostgreSQL 配置資料庫 + Nginx Reverse Proxy + TLS 支援。*
