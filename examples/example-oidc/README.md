# example-oidc

The embedded OAuth/JWT issuer (`r2e-oidc`): the app issues its own tokens and
validates them, with no external identity provider.

What it shows:

- An in-memory user store: `alice` / `password123` (roles `admin`, `user`) and
  `bob` / `password123` (role `user`)
- A client registry: `my-service` / `service-secret` with scopes `jobs:read`
  and `jobs:run` (client-credentials grant)
- Issuer `http://localhost:3000`, audience `r2e-app`
- Routes: `/public` (anonymous), `/me` (any authenticated user), `/admin`
  (`#[roles("admin")]`), plus `/oauth/token`, OpenAPI docs at `/docs`, health,
  CORS and HTTP tracing

The password grant is enabled with `enable_password_grant_for_development()`;
it is for local development only.

## Running

```bash
cargo run -p example-oidc
```

The startup log prints ready-to-paste commands:

```bash
curl -s -X POST localhost:3000/oauth/token \
  -d 'grant_type=password&username=alice&password=password123' | jq
curl -s localhost:3000/me -H 'Authorization: Bearer <token>' | jq
curl -s localhost:3000/admin -H 'Authorization: Bearer <token>' | jq
curl -s -X POST localhost:3000/oauth/token -u 'my-service:service-secret' \
  -d 'grant_type=client_credentials' | jq
```

`/admin` answers 403 for `bob`.
