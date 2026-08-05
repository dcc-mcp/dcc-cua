param(
    [Parameter(Mandatory = $true)]
    [string]$Binary
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$binaryCandidate = if ([System.IO.Path]::IsPathRooted($Binary)) { $Binary } else { Join-Path $repoRoot $Binary }
$binaryPath = (Resolve-Path $binaryCandidate).Path
$metadata = cargo metadata --format-version 1 --locked | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed" }
$sdkManifest = $metadata.packages |
    Where-Object { $_.name -eq "cua-driver-sdk" } |
    Select-Object -First 1 -ExpandProperty manifest_path
if (-not $sdkManifest) { throw "pinned cua-driver-sdk source was not resolved" }
$sdkDir = Split-Path -Parent $sdkManifest
$cratesDir = Split-Path -Parent $sdkDir
$rustDir = Split-Path -Parent $cratesDir
$cuaDriverDir = Split-Path -Parent $rustDir

$isWindowsHost = $env:OS -eq "Windows_NT"
$isMacHost = $PSVersionTable.PSVersion.Major -ge 6 -and $IsMacOS
if ($isWindowsHost) {
    & (Join-Path $cuaDriverDir "tests\fixtures\build\windows.ps1") -Targets electron,wpf
} elseif ($isMacHost) {
    & bash (Join-Path $cuaDriverDir "tests/fixtures/build/macos.sh") --only electron
    if ($LASTEXITCODE -eq 0) {
        & bash (Join-Path $cuaDriverDir "tests/fixtures/build/macos.sh") --only appkit
    }
} else {
    & bash (Join-Path $cuaDriverDir "tests/fixtures/build/linux.sh") --only "electron,gtk3"
}
if ($LASTEXITCODE -ne 0) { throw "official CUA GUI fixture build failed" }

$env:CUA_TEST_APPS_ROOT = Join-Path $rustDir "test-apps"
$env:DCC_CUA_E2E_BINARY = $binaryPath
cargo nextest run --locked -p dcc-cua-e2e --features gui-e2e --no-capture
if ($LASTEXITCODE -ne 0) { throw "GUI E2E failed" }
