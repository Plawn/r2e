# example-postgres

Full CRUD REST API with PostgreSQL demonstrating:

- `#[producer]` for `PgPool` with `#[config("database.url")]`
- `sqlx::migrate!()` in `on_start` hook
- Full CRUD (GET list paginated, GET by id, POST, PUT, DELETE)
- `#[managed] tx: &mut Tx<'_, Postgres>` for transactional writes
- `Pageable`/`Page` for paginated listings
- Custom `AppError` with `IntoResponse` + `From<sqlx::Error>`
- Automatic validation via `garde::Validate`
- `Entity` trait implementation

## Running

```bash
# Start PostgreSQL
docker compose up -d

# Run the app
cargo run -p example-postgres
```

The API is available at `http://localhost:3000`.

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
