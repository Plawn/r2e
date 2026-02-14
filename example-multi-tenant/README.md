# example-multi-tenant

Tenant isolation via JWT claims and custom guards, demonstrating:

- Custom identity type (`TenantUser`) via `ClaimsIdentity` + `impl_claims_identity_extractor!`
- Custom `Guard<AppState, TenantUser>` reading path params from `GuardContext`
- Layered auth: `#[guard(TenantGuard)]` + `#[roles("admin")]`
- Super-admin bypass in guard logic
- Per-tenant data filtering in service layer
- Mixed controller pattern (param-level `#[inject(identity)]`)
- SQLite in-memory database

## Running

```bash
cargo run -p example-multi-tenant
```

The API is available at `http://localhost:3000`. A test JWT is printed at startup.

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/tenants/{tenant_id}/projects` | List projects for tenant (guarded) |
| POST | `/tenants/{tenant_id}/projects` | Create project for tenant (guarded) |
| GET | `/admin/tenants` | List all tenants (super-admin only) |
| GET | `/health` | Health check |
