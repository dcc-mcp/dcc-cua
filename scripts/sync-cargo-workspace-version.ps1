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

function Update-TomlSectionVersion {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Section
    )

    $content = Get-Content -LiteralPath $Path -Raw
    $escapedSection = [regex]::Escape($Section)
    $pattern = '(?ms)(^\[' + $escapedSection + '\]\r?\n(?:(?!^\[).)*?^version\s*=\s*)"[^"]+"'
    $matches = [regex]::Matches($content, $pattern)
    if ($matches.Count -ne 1) {
        throw "expected exactly one version in [$Section] of $Path, found $($matches.Count)"
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
    Set-Content -LiteralPath $Path -Value $updated -NoNewline -Encoding utf8
}

$rootManifest = Join-Path $rootPath 'Cargo.toml'
Update-TomlSectionVersion -Path $rootManifest -Section 'workspace.package'

$metadata = (& cargo metadata --format-version 1 --no-deps | ConvertFrom-Json)
if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed with code $LASTEXITCODE"
}

$workspaceMembers = @($metadata.workspace_members)
$workspaceMemberNames = @(
    $metadata.packages |
        Where-Object { $workspaceMembers -contains $_.id } |
        Select-Object -ExpandProperty name
)

function Update-WorkspaceDependencyVersions {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string[]]$PackageNames
    )

    $content = Get-Content -LiteralPath $Path -Raw
    $updated = $content
    foreach ($packageName in $PackageNames) {
        $escapedName = [regex]::Escape($packageName)
        $pattern = '(?m)^(\s*' + $escapedName + '\s*=\s*\{[^\r\n]*\bversion\s*=\s*)"[^"]+"'
        $updated = [regex]::Replace(
            $updated,
            $pattern,
            [System.Text.RegularExpressions.MatchEvaluator]{
                param($match)
                return $match.Groups[1].Value + '"' + $Version + '"'
            }
        )
    }
    if ($updated -ne $content) {
        Set-Content -LiteralPath $Path -Value $updated -NoNewline -Encoding utf8
    }
}

foreach ($package in $metadata.packages) {
    if ($workspaceMemberNames -notcontains $package.name) {
        continue
    }
    $manifestPath = [System.IO.Path]::GetFullPath([string]$package.manifest_path)
    Update-TomlSectionVersion -Path $manifestPath -Section 'package'
    Update-WorkspaceDependencyVersions -Path $manifestPath -PackageNames $workspaceMemberNames
}

function Update-CargoLockVersions {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string[]]$PackageNames
    )

    $content = Get-Content -LiteralPath $Path -Raw
    $updated = $content
    foreach ($packageName in $PackageNames) {
        $escapedName = [regex]::Escape($packageName)
        $pattern = '(?ms)(^\[\[package\]\]\r?\nname = "' + $escapedName + '"\r?\nversion = )"[^"]+"'
        $matches = [regex]::Matches($updated, $pattern)
        if ($matches.Count -ne 1) {
            throw "expected exactly one Cargo.lock entry for $packageName, found $($matches.Count)"
        }
        $updated = [regex]::Replace(
            $updated,
            $pattern,
            [System.Text.RegularExpressions.MatchEvaluator]{
                param($match)
                return $match.Groups[1].Value + '"' + $Version + '"'
            },
            1
        )
    }
    if ($updated -ne $content) {
        Set-Content -LiteralPath $Path -Value $updated -NoNewline -Encoding utf8
    }
}

Update-CargoLockVersions -Path (Join-Path $rootPath 'Cargo.lock') -PackageNames $workspaceMemberNames

$updatedMetadata = (& cargo metadata --locked --format-version 1 --no-deps | ConvertFrom-Json)
if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed after version synchronization with code $LASTEXITCODE"
}
$versions = @(
    $updatedMetadata.packages |
        Where-Object { $workspaceMemberNames -contains $_.name } |
        Select-Object -ExpandProperty version -Unique
)
if ($versions.Count -ne 1 -or $versions[0] -ne $Version) {
    throw "workspace versions are not synchronized to ${Version}: $($versions -join ', ')"
}

Write-Output "synchronized $($workspaceMembers.Count) workspace packages to $Version"
