# Vendored openfga/language transformer corpus

DSL↔JSON pairs vendored from
[openfga/language](https://github.com/openfga/language)
`tests/data/transformer/` (Apache-2.0, see `LICENSE` in this directory).

Each case directory holds `authorization-model.fga` (DSL input) and
`authorization-model.json` (the JSON the official transformer produces).
`tests/parser.rs` parses every `.fga` and asserts value-equality with the
`.json`. Re-vendor by copying the upstream directory when new grammar
features need coverage.
