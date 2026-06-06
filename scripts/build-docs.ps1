param(
    [switch]$Serve,
    [int]$Port = 8080
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location -Path $repoRoot

Write-Host "==> Restoring DocFX tool"
dotnet tool restore
if ($LASTEXITCODE -ne 0) {
    throw "dotnet tool restore failed with exit code $LASTEXITCODE"
}

Write-Host "==> Building DocFX site"
dotnet docfx docfx.json --warningsAsErrors
if ($LASTEXITCODE -ne 0) {
    throw "dotnet docfx docfx.json --warningsAsErrors failed with exit code $LASTEXITCODE"
}

if ($Serve) {
    Write-Host "==> Serving docs at http://localhost:$Port"
    dotnet docfx serve docs/_site --port $Port
    if ($LASTEXITCODE -ne 0) {
        throw "dotnet docfx serve docs/_site --port $Port failed with exit code $LASTEXITCODE"
    }
}
