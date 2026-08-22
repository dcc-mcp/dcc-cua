[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\d+\.\d+\.\d+(?:[-+].*)?$')]
    [string]$Version,

    [string]$Root = (Get-Location).Path
)

$ErrorActionPreference = 'Stop'
$rootPath = [System.IO.Path]::GetFullPath($Root)
Set-Location $rootPath

$rootManifest = Join-Path $rootPath 'Cargo.toml'
$content = Get-Content -LiteralPath $rootManifest -Raw
$pattern = '(?ms)(^\[workspace\.package\]\r?\n(?:(?!^\[).)*?^version\s*=\s*)"[^"]+"'
$matches = [regex]::Matches($content, $pattern)
if ($matches.Count -ne 1) {
    throw "expected exactly one inherited workspace version in $rootManifest, found $($matches.Count)"
}
$updated = [regex]::Replace(
    $content,
    $pattern,
    [System.Text.RegularExpressions.MatchEvaluator]{
        param($match)
        return $match.Groups[1].Value + '"' + $Version + '"'
    },
    1
)
if ($updated -ne $content) {
    Set-Content -LiteralPath $rootManifest -Value $updated -NoNewline -Encoding utf8
}

# Cargo owns member inheritance and lockfile representation. Running metadata
# refreshes local package versions without rewriting every member manifest.
$metadata = (& cargo metadata --format-version 1 --no-deps | ConvertFrom-Json)
if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed with code $LASTEXITCODE"
}
$workspaceMembers = @($metadata.workspace_members)
$workspacePackages = @(
    $metadata.packages | Where-Object { $workspaceMembers -contains $_.id }
)
$versions = @(
    $workspacePackages | Select-Object -ExpandProperty version -Unique
)
if ($versions.Count -ne 1 -or $versions[0] -ne $Version) {
    throw "workspace versions are not inherited as ${Version}: $($versions -join ', ')"
}

& cargo metadata --locked --format-version 1 --no-deps | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Cargo.lock is not synchronized to the inherited workspace version"
}

Write-Output "synchronized $($workspacePackages.Count) inherited workspace packages to $Version"
