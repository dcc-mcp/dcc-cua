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
    if ($LASTEXITCODE -eq 0) {
        $macArchitecture = (& uname -m).Trim()
        if ($LASTEXITCODE -ne 0 -or $macArchitecture -notin @("arm64", "x86_64")) {
            throw "unsupported macOS runner architecture: $macArchitecture"
        }
        $macTarget = "$macArchitecture-apple-macos13.0"
        $appKitSourceDirectory = Join-Path $cuaDriverDir "tests/fixtures/apps/macos/appkit"
        $appKitSources = @(
            Get-ChildItem -LiteralPath $appKitSourceDirectory -Filter "*.swift" -File |
                Sort-Object -Property Name |
                Select-Object -ExpandProperty FullName
        )
        if ($appKitSources.Count -eq 0) {
            throw "official CUA AppKit fixture has no Swift sources"
        }
        $appKitExecutable = Join-Path $rustDir "test-apps/harness-appkit/CuaTestHarness.AppKit.app/Contents/MacOS/CuaTestHarness.AppKit"
        & xcrun swiftc -O -target $macTarget -parse-as-library -o $appKitExecutable @appKitSources
        if ($LASTEXITCODE -eq 0) {
            $fixtureArchitecture = (& file $appKitExecutable | Out-String).Trim()
            if ($LASTEXITCODE -ne 0 -or $fixtureArchitecture -notmatch [regex]::Escape($macArchitecture)) {
                throw "official CUA AppKit fixture does not match runner architecture ${macArchitecture}: $fixtureArchitecture"
            }
        }
    }
} else {
    & bash (Join-Path $cuaDriverDir "tests/fixtures/build/linux.sh") --only "electron,gtk3"
}
if ($LASTEXITCODE -ne 0) { throw "official CUA GUI fixture build failed" }

$env:CUA_TEST_APPS_ROOT = Join-Path $rustDir "test-apps"
$env:DCC_CUA_E2E_BINARY = $binaryPath

cargo nextest run --locked -p dcc-cua-e2e --features gui-e2e --no-capture
if ($LASTEXITCODE -ne 0) { throw "GUI E2E failed" }

& (Join-Path $PSScriptRoot "test-host-jsonl-output-recovery.ps1") -Binary $binaryPath
