# example-multi-tenant

**Row-level tenancy.** One shared database, rows tagged with a tenant column,
isolation enforced by a custom guard reading the tenant off the JWT identity.
No framework tenancy layer involved — this is the model you reach for when
every tenant's data lives in the same tables.

For the other model — one database *per tenant*, resolved from the request and
provisioned on first use by the `tenant` feature (`Tenant<T>`, `Tenanted<T>`,
`TenantTx`) — see
[`examples/example-multi-tenant-db`](../example-multi-tenant-db/README.md), and
[`docs/features/24-tenancy.md`](../../docs/features/24-tenancy.md) for the
guide covering both.

Tenant isolation via JWT claims and custom guards, demonstrating:

- Custom identity type (`TenantUser`) via `FromValidatedJwtClaims` + `impl_claims_identity_extractor!`
- Custom `Guard<TenantUser>` reading path params from `GuardContext`
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
