# example-postgres

Full CRUD REST API with PostgreSQL demonstrating:

- `SqlxDataSource` plugin: pool + migrations from the `datasource.*` config
- `sqlx::migrate!()` applied at boot via `datasource.migrate-at-start`
- Full CRUD (GET list paginated, GET by id, POST, PUT, DELETE)
- `DbPool<Postgres>` injected into a service bean (the rotating pool is the executor)
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

Each test provisions an isolated database on the shared container and points
the app at it with `override_config_value("datasource.url", ...)`. Migrations
need no help from the test: the datasource plugin runs them inside
`build_state()`, which `TestApp` executes.

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
