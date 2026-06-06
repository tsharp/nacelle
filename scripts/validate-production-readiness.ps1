$ErrorActionPreference = "Stop"

function Invoke-Step {
    param(
        [string] $Name,
        [scriptblock] $Command
    )

    Write-Host "==> $Name"
    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Name failed with exit code $LASTEXITCODE"
    }
}

Invoke-Step "cargo fmt" { cargo fmt --all -- --check }
Invoke-Step "workspace tests" { cargo test --workspace --all-targets }
Invoke-Step "workspace clippy" { cargo clippy --workspace --all-targets -- -D warnings }
Invoke-Step "nacelle-core full tests" { cargo test -p nacelle-core --features http-types,tls-self-signed,tower,otel --all-targets }
Invoke-Step "nacelle-core full clippy" { cargo clippy -p nacelle-core --features http-types,tls-self-signed,tower,otel --all-targets -- -D warnings }
Invoke-Step "nacelle-tcp tests" { cargo test -p nacelle-tcp --all-targets }
Invoke-Step "nacelle-tcp clippy" { cargo clippy -p nacelle-tcp --all-targets -- -D warnings }
Invoke-Step "nacelle-tcp tls tests" { cargo test -p nacelle-tcp --features tls-self-signed --all-targets }
Invoke-Step "nacelle-tcp tls clippy" { cargo clippy -p nacelle-tcp --features tls-self-signed --all-targets -- -D warnings }
Invoke-Step "nacelle-http full tests" { cargo test -p nacelle-http --features tls-self-signed --all-targets }
Invoke-Step "nacelle-http full clippy" { cargo clippy -p nacelle-http --features tls-self-signed --all-targets -- -D warnings }
Invoke-Step "nacelle full tests" { cargo test -p nacelle --features reference_protocol,http,tower,otel --all-targets }
Invoke-Step "nacelle full clippy" { cargo clippy -p nacelle --features reference_protocol,http,tower,otel --all-targets -- -D warnings }
Invoke-Step "nacelle http tests" { cargo test -p nacelle --no-default-features --features http --all-targets }
Invoke-Step "nacelle tls tests" { cargo test -p nacelle --no-default-features --features tls --all-targets }
Invoke-Step "nacelle self-signed tls tests" { cargo test -p nacelle --no-default-features --features tls-self-signed --all-targets }
Invoke-Step "nacelle https self-signed tests" { cargo test -p nacelle --no-default-features --features http,tls-self-signed --all-targets }
Invoke-Step "nacelle raw tcp self-signed tests" { cargo test -p nacelle --features reference_protocol,tls-self-signed --all-targets }
Invoke-Step "nacelle all-feature clippy" { cargo clippy -p nacelle --features reference_protocol,http,tower,otel,tls-self-signed --all-targets -- -D warnings }
Invoke-Step "nacelle no-default tests" { cargo test -p nacelle --no-default-features --all-targets }
Invoke-Step "stress server tests" { cargo test -p nacelle-stress-server --all-targets }

cargo tree -i serde_yaml
if ($LASTEXITCODE -eq 0) {
    throw "serde_yaml is still present"
}

cargo tree -i unsafe-libyaml
if ($LASTEXITCODE -eq 0) {
    throw "unsafe-libyaml is still present"
}

if (Get-Command cargo-audit -ErrorAction SilentlyContinue) {
    Invoke-Step "cargo audit" { cargo audit }
} else {
    Write-Warning "cargo-audit not installed; skipping audit"
}
