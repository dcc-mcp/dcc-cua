param(
    [Parameter(Mandatory = $true)]
    [string]$Binary
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$binaryPath = (Resolve-Path -LiteralPath $Binary).Path

function Invoke-BinaryJson {
    param([string[]]$Arguments)

    $output = & $binaryPath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "dcc-mcp-cua exited with code ${LASTEXITCODE}: $($Arguments -join ' ')"
    }
    try {
        return ($output | Out-String | ConvertFrom-Json)
    }
    catch {
        throw "dcc-mcp-cua returned invalid JSON for '$($Arguments[0])': $_"
    }
}

$help = & $binaryPath --help | Out-String
if ($LASTEXITCODE -ne 0 -or $help -notmatch "host-batch") {
    throw "release CLI help smoke test failed"
}

$manifest = Invoke-BinaryJson -Arguments @("manifest")
$expectedOs = if ($IsWindows) { "windows" } elseif ($IsMacOS) { "macos" } else { "linux" }
if ($manifest.name -ne "dcc-mcp-cua" -or $manifest.target.os -ne $expectedOs) {
    throw "manifest does not describe the current release binary"
}
if ($manifest.version -notmatch '^\d+\.\d+\.\d+$' -or $manifest.host.protocol_version -ne 1) {
    throw "manifest version or Host protocol is invalid"
}
if ($manifest.host.snapshot_transports -notcontains "shared_memory" -or
    $manifest.host.capabilities -notcontains "two_axis_scroll") {
    throw "manifest omitted required Host capabilities"
}

$batchRequest = @(
    [ordered]@{ request_id = "e2e-apps"; method = "list_apps"; params = @{} },
    [ordered]@{ request_id = "e2e-tools"; method = "list_tools"; params = @{} }
) | ConvertTo-Json -Depth 4 -Compress
$responses = @(Invoke-BinaryJson -Arguments @(
    "host-batch",
    "--spawn", $binaryPath,
    "--snapshot-transport", "shared_memory",
    "--json", $batchRequest
))

if ($responses.Count -ne 2 -or
    $responses[0].request_id -ne "e2e-apps" -or
    $responses[1].request_id -ne "e2e-tools") {
    throw "Host batch IPC did not preserve response order and correlation IDs"
}
if ($responses[0].type -ne "apps" -or $null -eq $responses[0].apps) {
    throw "embedded CUA application inventory failed"
}
if ($responses[1].type -ne "tools" -or $null -eq $responses[1].tools) {
    throw "embedded CUA tool inventory failed"
}

$toolNames = @($responses[1].tools.tools | ForEach-Object { $_.name })
foreach ($requiredTool in @(
    "list_apps",
    "list_windows",
    "click",
    "scroll",
    "get_desktop_state",
    "start_session"
)) {
    if ($toolNames -notcontains $requiredTool) {
        throw "embedded CUA tool inventory omitted $requiredTool"
    }
}

Write-Host "CLI E2E passed for ${expectedOs}: manifest, release binary, Host IPC, apps, and $($toolNames.Count) CUA tools."
