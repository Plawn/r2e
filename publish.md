Ordre de publication

# 1. Crates sans dépendances internes
cargo publish -p quarlus-macros
cargo publish -p quarlus-events
cargo publish -p quarlus-cache

# 2. quarlus-core (dépend de quarlus-macros)
cargo publish -p quarlus-core

# 3. Crates qui dépendent de quarlus-core
cargo publish -p quarlus-security
cargo publish -p quarlus-scheduler
cargo publish -p quarlus-data
cargo publish -p quarlus-rate-limit
cargo publish -p quarlus-openapi

# 4. Crates qui dépendent de plusieurs
cargo publish -p quarlus-utils      # dépend de core + cache
cargo publish -p quarlus-test       # dépend de core + security

# 5. CLI (optionnel)
cargo publish -p quarlus-cli