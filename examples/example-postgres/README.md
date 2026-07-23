# example-postgres

Full CRUD REST API with PostgreSQL demonstrating:

- `#[producer]` for `PgPool` with `#[config("database.url")]`
- `sqlx::migrate!()` in `on_start` hook
- Full CRUD (GET list paginated, GET by id, POST, PUT, DELETE)
- `#[managed] tx: &mut Tx<'_, Postgres>` for transactional writes
- `Pageable`/`Page` for paginated listings
- Custom `HttpError` with `IntoResponse` + `From<sqlx::Error>`
- Automatic validation via `garde::Validate`
- Plain SQLx row models without an extra repository abstraction

## Running

```bash
# Start PostgreSQL
docker compose up -d

# Run the app
cargo run -p example-postgres
```

The API is available at `http://localhost:3000`.

## Testing

Integration tests (`tests/postgres_test.rs`) boot the real app against a
throwaway PostgreSQL container via `DevPostgres` (dev services) — no local
Postgres needed, just a running Docker daemon. They are `#[ignore]`d by default
so a Docker-less CI stays green:

```bash
cargo test -p example-postgres --test postgres_test -- --ignored
```

Each test provisions an isolated database on the shared container and applies
the migrations itself (the app's `on_start` migration hook runs only on the
serve path, not under `TestApp`), then points the app at it with
`override_config_value("database.url", ...)`.

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/articles` | List articles (paginated: `?page=0&size=20`) |
| GET | `/articles/{id}` | Get article by ID |
| POST | `/articles` | Create article |
| PUT | `/articles/{id}` | Update article |
| DELETE | `/articles/{id}` | Delete article |
| GET | `/health` | Health check |
| GET | `/openapi.json` | OpenAPI spec |
| GET | `/docs` | Interactive API docs |
