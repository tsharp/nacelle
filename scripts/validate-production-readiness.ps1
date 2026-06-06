$ErrorActionPreference = "Stop"

cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p nacelle-core --features http-types,tls-self-signed,tower,otel --all-targets
cargo clippy -p nacelle-core --features http-types,tls-self-signed,tower,otel --all-targets -- -D warnings
cargo test -p nacelle-tcp --all-targets
cargo clippy -p nacelle-tcp --all-targets -- -D warnings
cargo test -p nacelle-http --features tls-self-signed --all-targets
cargo clippy -p nacelle-http --features tls-self-signed --all-targets -- -D warnings
cargo test -p nacelle --features reference_protocol,http,tower,otel --all-targets
cargo clippy -p nacelle --features reference_protocol,http,tower,otel --all-targets -- -D warnings
cargo test -p nacelle --no-default-features --features http --all-targets
cargo test -p nacelle --no-default-features --features tls --all-targets
cargo test -p nacelle --no-default-features --features tls-self-signed --all-targets
cargo test -p nacelle --no-default-features --features http,tls-self-signed --all-targets
cargo clippy -p nacelle --features reference_protocol,http,tower,otel,tls-self-signed --all-targets -- -D warnings
cargo test -p nacelle --no-default-features --all-targets
cargo test -p nacelle-stress-server --all-targets

cargo tree -i serde_yaml
if ($LASTEXITCODE -eq 0) {
    throw "serde_yaml is still present"
}

cargo tree -i unsafe-libyaml
if ($LASTEXITCODE -eq 0) {
    throw "unsafe-libyaml is still present"
}

if (Get-Command cargo-audit -ErrorAction SilentlyContinue) {
    cargo audit
} else {
    Write-Warning "cargo-audit not installed; skipping audit"
}
