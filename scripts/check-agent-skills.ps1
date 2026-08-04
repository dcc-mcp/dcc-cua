[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$skillsRoot = Join-Path $PSScriptRoot "..\skills"
$skillDirectories = @(Get-ChildItem -LiteralPath $skillsRoot -Directory)
if ($skillDirectories.Count -eq 0) {
    throw "No Agent Skills found under $skillsRoot"
}

foreach ($directory in $skillDirectories) {
    $skillPath = Join-Path $directory.FullName "SKILL.md"
    if (-not (Test-Path -LiteralPath $skillPath -PathType Leaf)) {
        throw "Missing SKILL.md: $skillPath"
    }
    $content = Get-Content -LiteralPath $skillPath -Raw
    if ($content -notmatch "(?m)^name:\s+$([regex]::Escape($directory.Name))\s*$") {
        throw "Skill name does not match directory: $skillPath"
    }
    foreach ($field in @("description:", "metadata:", "  dcc-mcp:")) {
        if ($content -notmatch "(?m)^$([regex]::Escape($field))") {
            throw "Missing frontmatter field '$field': $skillPath"
        }
    }
    if ($content -match "(?m)^version:\s") {
        throw "Version must live under metadata.dcc-mcp: $skillPath"
    }
}

Write-Output "Validated $($skillDirectories.Count) Agent Skills."
