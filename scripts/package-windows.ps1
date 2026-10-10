$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot
$buildDir = Join-Path $projectRoot "build"

Push-Location $projectRoot
try {
    cargo build --release
    New-Item -ItemType Directory -Path $buildDir -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $projectRoot "target\release\cli-launcher.exe") -Destination (Join-Path $buildDir "CliLauncher.exe") -Force
    Write-Host "Created: $buildDir\CliLauncher.exe"
}
finally {
    Pop-Location
}
