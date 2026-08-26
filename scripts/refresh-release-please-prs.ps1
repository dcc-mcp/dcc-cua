[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Repository
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$components = @(
    [pscustomobject]@{
        Name = 'dcc-cua'
        Branch = 'release-please--branches--main--components--dcc-cua'
        ManifestKey = '.'
        Files = @('CHANGELOG.md', 'version.txt')
        SyncCargo = $true
    },
    [pscustomobject]@{
        Name = 'dcc-cua-browser-extension'
        Branch = 'release-please--branches--main--components--dcc-cua-browser-extension'
        ManifestKey = 'browser-extension/chrome'
        Files = @(
            'browser-extension/chrome/CHANGELOG.md',
            'browser-extension/chrome/component-manifest.json',
            'browser-extension/chrome/package-lock.json',
            'browser-extension/chrome/package.json'
        )
        SyncCargo = $false
    }
)

$pullRequests = gh pr list --repo $Repository --state open --json number,headRefName --limit 20 |
    ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw "release PR query failed with code $LASTEXITCODE" }

$baseBranch = 'main'
$baseRef = "refs/remotes/origin/$baseBranch"
$expectedBaseCommit = (git rev-parse --verify 'HEAD^{commit}').Trim()
if ($LASTEXITCODE -ne 0) { throw "current release refresh base resolution failed with code $LASTEXITCODE" }
if ($env:GITHUB_SHA -and $env:GITHUB_SHA -ne $expectedBaseCommit) {
    throw "checkout HEAD $expectedBaseCommit does not match workflow base $env:GITHUB_SHA"
}

git config user.name 'github-actions[bot]'
git config user.email '41898282+github-actions[bot]@users.noreply.github.com'

foreach ($component in $components) {
    $branch = $component.Branch
    $pullRequest = $pullRequests |
        Where-Object { $_.headRefName -eq $branch } |
        Select-Object -First 1
    if (-not $pullRequest) {
        Write-Host "No pending $($component.Name) release PR."
        continue
    }

    git fetch --no-tags --depth=1 origin "+refs/heads/${baseBranch}:${baseRef}"
    if ($LASTEXITCODE -ne 0) { throw "$baseBranch fetch failed with code $LASTEXITCODE" }
    $baseCommit = (git rev-parse --verify "${baseRef}^{commit}").Trim()
    if ($LASTEXITCODE -ne 0) { throw "$baseBranch resolution failed with code $LASTEXITCODE" }
    if ($baseCommit -ne $expectedBaseCommit) {
        throw "fetched $baseBranch commit $baseCommit does not match expected base $expectedBaseCommit"
    }

    $releaseRef = "refs/remotes/origin/$branch"
    git fetch origin "+refs/heads/${branch}:${releaseRef}"
    if ($LASTEXITCODE -ne 0) { throw "$branch fetch failed with code $LASTEXITCODE" }
    $releaseHead = (git rev-parse $releaseRef).Trim()
    if ($LASTEXITCODE -ne 0) { throw "$branch resolution failed with code $LASTEXITCODE" }

    git checkout -B $branch $baseCommit
    if ($LASTEXITCODE -ne 0) { throw "$branch reset failed with code $LASTEXITCODE" }

    $releaseManifestJson = (& git show "${releaseRef}:.release-please-manifest.json") -join "`n"
    if ($LASTEXITCODE -ne 0) { throw "$branch release manifest read failed with code $LASTEXITCODE" }
    $releaseManifest = $releaseManifestJson | ConvertFrom-Json
    $baseManifest = Get-Content .release-please-manifest.json -Raw | ConvertFrom-Json
    $manifestKey = $component.ManifestKey
    $componentVersion = $releaseManifest.PSObject.Properties[$manifestKey].Value
    if (-not $componentVersion) { throw "$branch release manifest omitted $manifestKey" }
    $baseProperty = $baseManifest.PSObject.Properties[$manifestKey]
    if (-not $baseProperty) { throw "main release manifest omitted $manifestKey" }
    $baseProperty.Value = $componentVersion
    $baseManifest | ConvertTo-Json | Set-Content .release-please-manifest.json -Encoding utf8NoBOM

    $restoreArguments = @('checkout', $releaseRef, '--') + @($component.Files)
    & git @restoreArguments
    if ($LASTEXITCODE -ne 0) { throw "$branch release file restore failed with code $LASTEXITCODE" }

    $stageFiles = @('.release-please-manifest.json') + @($component.Files)
    if ($component.SyncCargo) {
        $manifest = Get-Content .release-please-manifest.json -Raw | ConvertFrom-Json
        $version = $manifest.'.'
        & pwsh -NoProfile -File scripts/sync-cargo-workspace-version.ps1 -Version $version
        if ($LASTEXITCODE -ne 0) { throw "$branch workspace version sync failed with code $LASTEXITCODE" }
        $stageFiles += @('Cargo.toml', 'Cargo.lock')
    }

    & git add -- @stageFiles
    if ($LASTEXITCODE -ne 0) { throw "$branch staging failed with code $LASTEXITCODE" }
    if (git diff --cached --quiet) {
        Write-Host "$branch is already based on current main."
    } else {
        git commit -m "chore: refresh $($component.Name) release branch from main"
        if ($LASTEXITCODE -ne 0) { throw "$branch commit failed with code $LASTEXITCODE" }
    }

    $lease = "refs/heads/${branch}:${releaseHead}"
    git push "--force-with-lease=$lease" origin "HEAD:refs/heads/$branch"
    if ($LASTEXITCODE -ne 0) { throw "$branch update failed with code $LASTEXITCODE" }
    gh workflow run ci-checks.yml --repo $Repository --ref $branch
    if ($LASTEXITCODE -ne 0) { throw "$branch CI dispatch failed with code $LASTEXITCODE" }
}
