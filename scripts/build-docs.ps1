param(
    [switch]$Open
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location -Path $repoRoot

Write-Host "==> Building Rust API documentation"
$previousRustdocFlags = $env:RUSTDOCFLAGS
$env:RUSTDOCFLAGS = (($previousRustdocFlags, "-D warnings") | Where-Object { $_ } | Select-Object -Unique) -join " "

try {
    cargo doc --workspace --all-features --no-deps
    if ($LASTEXITCODE -ne 0) {
        throw "cargo doc --workspace --all-features --no-deps failed with exit code $LASTEXITCODE"
    }
} finally {
    $env:RUSTDOCFLAGS = $previousRustdocFlags
}

$indexPath = Join-Path $repoRoot "target\doc\nacelle\index.html"
Write-Host "==> API docs: $indexPath"

if ($Open) {
    Invoke-Item -Path $indexPath
}
