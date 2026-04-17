param(
    [string]$Bind = "127.0.0.1:7878",
    [int]$ServerThreads = [Environment]::ProcessorCount,
    [int]$DurationSecs = 30,
    [int]$Connections = 256,
    [int]$Pipeline = 8,
    [int]$PayloadBytes = 256,
    [switch]$Debug
)

$ErrorActionPreference = "Stop"

Set-Location -Path $PSScriptRoot

function Invoke-Cargo {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Args
    )

    & cargo @Args
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($Args -join ' ') failed with exit code $LASTEXITCODE"
    }
}

function Wait-PortOpen {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Bind,
        [int]$TimeoutMs = 5000
    )

    $idx = $Bind.LastIndexOf(':')
    if ($idx -lt 1) {
        throw "Invalid --Bind '$Bind'. Expected host:port"
    }

    $hostName = $Bind.Substring(0, $idx)
    $port = [int]$Bind.Substring($idx + 1)
    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)

    while ([DateTime]::UtcNow -lt $deadline) {
        $client = New-Object System.Net.Sockets.TcpClient
        try {
            $task = $client.ConnectAsync($hostName, $port)
            if ($task.Wait(100) -and $client.Connected) {
                return
            }
        } catch {
            # Ignore while waiting for the server to begin listening.
        } finally {
            $client.Dispose()
        }

        Start-Sleep -Milliseconds 100
    }

    throw "Timed out waiting for $Bind to accept connections"
}

$profile = if ($Debug) { "debug" } else { "release" }
$cargoProfileArg = if ($Debug) { @() } else { @("--release") }

Write-Host "==> Building nacelle-stress-test ($profile)"
Invoke-Cargo -Args (@("build") + $cargoProfileArg + @("--package", "nacelle-stress-test"))

Write-Host "==> Building kimojio-iocp-server ($profile)"
Invoke-Cargo -Args (@(
    "build"
) + $cargoProfileArg + @(
    "--package", "nacelle-stress-server",
    "--bin", "kimojio-iocp-server",
    "--no-default-features",
    "--features", "kimojio-iocp-runtime"
))

$serverExe = Join-Path $PSScriptRoot "target\$profile\kimojio-iocp-server.exe"
$clientExe = Join-Path $PSScriptRoot "target\$profile\nacelle-stress-test.exe"

if (-not (Test-Path -Path $serverExe)) {
    throw "Missing server binary: $serverExe"
}
if (-not (Test-Path -Path $clientExe)) {
    throw "Missing stress client binary: $clientExe"
}

$serverArgs = @(
    "--bind", $Bind,
    "--server-threads", "$ServerThreads"
)

$server = $null
try {
    Write-Host "==> Starting server: $serverExe $($serverArgs -join ' ')"
    $server = Start-Process -FilePath $serverExe -ArgumentList $serverArgs -PassThru

    Wait-PortOpen -Bind $Bind -TimeoutMs 5000

    Write-Host "--- kimojio iocp threads=$ServerThreads connections=$Connections pipeline=$Pipeline ---"

    $clientArgs = @(
        "--addr", $Bind,
        "--connections", "$Connections",
        "--pipeline", "$Pipeline",
        "--duration-secs", "$DurationSecs",
        "--payload-bytes", "$PayloadBytes"
    )
    & $clientExe @clientArgs

    if ($LASTEXITCODE -ne 0) {
        throw "nacelle-stress-test exited with code $LASTEXITCODE"
    }
}
finally {
    if ($null -ne $server -and -not $server.HasExited) {
        Write-Host "==> Stopping server (PID $($server.Id))"
        Stop-Process -Id $server.Id -Force
    }
}