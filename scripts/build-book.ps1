param(
    [switch]$Serve,
    [int]$Port = 3000,
    [switch]$Open
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location -Path $repoRoot

if (-not (Get-Command mdbook -ErrorAction SilentlyContinue)) {
    Write-Host "==> Installing mdBook"
    cargo install mdbook --locked
    if ($LASTEXITCODE -ne 0) {
        throw "cargo install mdbook --locked failed with exit code $LASTEXITCODE"
    }
}

Write-Host "==> Building mdBook documentation"
mdbook build
if ($LASTEXITCODE -ne 0) {
    throw "mdbook build failed with exit code $LASTEXITCODE"
}

$indexPath = Join-Path $repoRoot "docs\book\index.html"
Write-Host "==> Book docs: $indexPath"

if ($Open) {
    Invoke-Item -Path $indexPath
}

if ($Serve) {
    Write-Host "==> Serving book docs at http://localhost:$Port"
    mdbook serve --hostname 127.0.0.1 --port $Port
    if ($LASTEXITCODE -ne 0) {
        throw "mdbook serve failed with exit code $LASTEXITCODE"
    }
}
