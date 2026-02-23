Ordre de publication

# 1. Crates sans dépendances internes
cargo publish -p r2e-macros
cargo publish -p r2e-events
cargo publish -p r2e-cache

# 2. r2e-core (dépend de r2e-macros)
cargo publish -p r2e-core

# 3. Crates qui dépendent de r2e-core
cargo publish -p r2e-security
cargo publish -p r2e-scheduler
cargo publish -p r2e-data
cargo publish -p r2e-rate-limit
cargo publish -p r2e-openapi

# 4. Crates qui dépendent de plusieurs
cargo publish -p r2e-utils      # dépend de core + cache
cargo publish -p r2e-test       # dépend de core + security

# 5. CLI (optionnel)
cargo publish -p r2e-cli