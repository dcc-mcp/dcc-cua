param(
    [string]$Destination = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$metadataJson = cargo metadata --locked --format-version 1 | Out-String
if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed with code $LASTEXITCODE"
}
$metadata = $metadataJson | ConvertFrom-Json
$sdk = @($metadata.packages | Where-Object { $_.name -eq "cua-driver-sdk" })
if ($sdk.Count -ne 1) {
    throw "expected exactly one pinned cua-driver-sdk package, found $($sdk.Count)"
}
$cli = @($metadata.packages | Where-Object { $_.name -eq "dcc-mcp-cua-cli" })
if ($cli.Count -ne 1) {
    throw "expected exactly one dcc-mcp-cua-cli package, found $($cli.Count)"
}
if ([string]::IsNullOrWhiteSpace($Destination)) {
    $Destination = "target/release/libexec/dcc-mcp-cua/$($cli[0].version)"
}

$rustRoot = Split-Path (Split-Path (Split-Path $sdk[0].manifest_path -Parent) -Parent) -Parent
$workspaceManifest = Join-Path $rustRoot "Cargo.toml"
$targetDirectory = Join-Path $repoRoot "target"
$packages = @("cua-driver", "cursor-theme-cli")
if ($env:OS -eq "Windows_NT") {
    $packages += "cua-driver-uia"
}
$cargoArguments = @("build", "--release", "--locked", "--manifest-path", $workspaceManifest, "--target-dir", $targetDirectory)
foreach ($package in $packages) {
    $cargoArguments += @("--package", $package)
}
cargo @cargoArguments
if ($LASTEXITCODE -ne 0) {
    throw "official CUA companion build failed with code $LASTEXITCODE"
}

$destinationRoot = if ([System.IO.Path]::IsPathRooted($Destination)) {
    [System.IO.Path]::GetFullPath($Destination)
} else {
    [System.IO.Path]::GetFullPath((Join-Path $repoRoot $Destination))
}
[void](New-Item -ItemType Directory -Path $destinationRoot -Force)
$suffix = if ($env:OS -eq "Windows_NT") { ".exe" } else { "" }
$binaryNames = @("cua-driver$suffix", "cua-cursor-theme$suffix")
if ($env:OS -eq "Windows_NT") {
    $binaryNames += "cua-driver-uia.exe"
}
foreach ($binaryName in $binaryNames) {
    $builtBinary = Join-Path $targetDirectory "release/$binaryName"
    $destinationBinary = Join-Path $destinationRoot $binaryName
    if ($builtBinary -ne $destinationBinary) {
        Copy-Item -LiteralPath $builtBinary -Destination $destinationBinary -Force
    }
}

$cuaRoot = Split-Path (Split-Path (Split-Path $rustRoot -Parent) -Parent) -Parent
Copy-Item -LiteralPath (Join-Path $cuaRoot "LICENSE.md") -Destination (Join-Path $destinationRoot "CUA-LICENSE.md") -Force
Write-Output "Built pinned official CUA companions in $destinationRoot"
