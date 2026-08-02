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
    $manifest.host.capabilities -notcontains "two_axis_scroll" -or
    $manifest.host.capabilities -notcontains "host_ping" -or
    $manifest.host.capabilities -notcontains "host_diagnostics") {
    throw "manifest omitted required Host capabilities"
}

$batchRequest = @(
    [ordered]@{ request_id = "e2e-ping"; method = "ping"; params = @{} },
    [ordered]@{ request_id = "e2e-apps"; method = "list_apps"; params = @{} },
    [ordered]@{ request_id = "e2e-tools"; method = "list_tools"; params = @{} }
) | ConvertTo-Json -Depth 4 -Compress
$responses = @(Invoke-BinaryJson -Arguments @(
    "host-batch",
    "--spawn", $binaryPath,
    "--snapshot-transport", "shared_memory",
    "--json", $batchRequest
))

if ($responses.Count -ne 3 -or
    $responses[0].request_id -ne "e2e-ping" -or
    $responses[1].request_id -ne "e2e-apps" -or
    $responses[2].request_id -ne "e2e-tools") {
    throw "Host batch IPC did not preserve response order and correlation IDs"
}
if ($responses[0].type -ne "pong" -or $responses[0].protocol_version -ne 1) {
    throw "Host ping failed"
}
if ($responses[1].type -ne "apps" -or $null -eq $responses[1].apps) {
    throw "embedded CUA application inventory failed"
}
if ($responses[2].type -ne "tools" -or $null -eq $responses[2].tools) {
    throw "embedded CUA tool inventory failed"
}

$toolNames = @($responses[2].tools.tools | ForEach-Object { $_.name })
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

$startInfo = [System.Diagnostics.ProcessStartInfo]::new()
$startInfo.FileName = $binaryPath
$startInfo.UseShellExecute = $false
$startInfo.RedirectStandardInput = $true
$startInfo.RedirectStandardOutput = $true
$startInfo.RedirectStandardError = $true
foreach ($argument in @(
    "host-jsonl",
    "--spawn", $binaryPath,
    "--parallel-discovery",
    "--snapshot-transport", "shared_memory"
)) {
    [void]$startInfo.ArgumentList.Add($argument)
}

$stream = [System.Diagnostics.Process]::new()
$stream.StartInfo = $startInfo
if (-not $stream.Start()) {
    throw "failed to start host-jsonl E2E process"
}
try {
    foreach ($request in @(
        '{"request_id":"stream-ping-1","method":"ping","params":{}}',
        '{"request_id":"stream-doctor","method":"doctor","params":{}}',
        '{"request_id":"stream-desktop-open","method":"open_desktop_session","params":{"session_id":"cli-e2e-lifecycle","grant":{"task_grant_id":"cli-e2e","dcc_type":"desktop","allow_raw_input":false}}}',
        '{"request_id":"stream-desktop-stop","method":"stop_desktop_session","params":{"session_id":"cli-e2e-lifecycle"}}',
        '{"request_id":"stream-error","method":"unknown_method","params":{}}',
        '{"request_id":"stream-ping-2","method":"ping","params":{}}'
    )) {
        $stream.StandardInput.WriteLine($request)
    }
    $stream.StandardInput.Flush()

    $streamResponses = @()
    foreach ($index in 0..5) {
        $read = $stream.StandardOutput.ReadLineAsync()
        if (-not $read.Wait(30000)) {
            throw "host-jsonl response $index timed out"
        }
        $streamResponses += ($read.Result | ConvertFrom-Json)
    }
    $stream.StandardInput.Close()
    if (-not $stream.WaitForExit(30000)) {
        $stream.Kill($true)
        throw "host-jsonl did not exit after stdin closed"
    }
    if ($stream.ExitCode -ne 0) {
        throw "host-jsonl failed: $($stream.StandardError.ReadToEnd())"
    }
}
finally {
    if (-not $stream.HasExited) {
        $stream.Kill($true)
    }
    $stream.Dispose()
}

$streamJson = $streamResponses | ConvertTo-Json -Depth 12 -Compress
if ($streamResponses[0].request_id -ne "stream-ping-1" -or
    $streamResponses[0].type -ne "pong" -or
    $streamResponses[1].request_id -ne "stream-doctor" -or
    $streamResponses[1].type -ne "diagnostics" -or
    $streamResponses[1].schema_version -ne 1 -or
    $null -eq $streamResponses[1].checks.driver.success -or
    $null -eq $streamResponses[1].checks.health.success -or
    $streamResponses[4].request_id -ne "stream-error" -or
    $streamResponses[4].type -ne "error" -or
    $streamResponses[5].request_id -ne "stream-ping-2" -or
    $streamResponses[5].type -ne "pong") {
    throw "host-jsonl did not preserve diagnostics, correlation, or error recovery: $streamJson"
}

if ($streamResponses[2].type -eq "desktop_session_opened") {
    if ($streamResponses[2].request_id -ne "stream-desktop-open" -or
        $streamResponses[2].started.active -ne $true -or
        $streamResponses[3].request_id -ne "stream-desktop-stop" -or
        $streamResponses[3].type -ne "desktop_session_stopped" -or
        $streamResponses[3].result.active -ne $false) {
        throw "Host desktop session lifecycle failed: $streamJson"
    }
}
elseif (-not $IsMacOS -or
    $streamResponses[1].ready -ne $false -or
    $streamResponses[2].request_id -ne "stream-desktop-open" -or
    $streamResponses[2].type -ne "error" -or
    [string]::IsNullOrWhiteSpace($streamResponses[2].code) -or
    $streamResponses[3].request_id -ne "stream-desktop-stop" -or
    $streamResponses[3].type -ne "error" -or
    [string]::IsNullOrWhiteSpace($streamResponses[3].code)) {
    throw "Host session lifecycle was not available without a structured macOS readiness refusal: $streamJson"
}

Write-Host "CLI E2E passed for ${expectedOs}: manifest, diagnostics, session lifecycle, batch/stream Host IPC, error recovery, apps, and $($toolNames.Count) CUA tools."
