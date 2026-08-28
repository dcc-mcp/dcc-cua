param(
    [Parameter(Mandatory = $true)]
    [string]$Binary
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$binaryPath = (Resolve-Path -LiteralPath $Binary).Path
$isWindowsHost = $env:OS -eq "Windows_NT"
$isMacHost = $false
if ($PSVersionTable.PSVersion.Major -ge 6) {
    $isMacHost = [bool]$IsMacOS
}
$endpointRuntimeDir = $null
$originalXdgRuntimeDir = $env:XDG_RUNTIME_DIR
$profileStore = $null
$profilePackageCopy = $null

function Invoke-BinaryJson {
    param([string[]]$Arguments)

    $output = & $binaryPath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "dcc-cua exited with code ${LASTEXITCODE}: $($Arguments -join ' ')"
    }
    try {
        return ($output | Out-String | ConvertFrom-Json)
    }
    catch {
        throw "dcc-cua returned invalid JSON for '$($Arguments[0])': $_"
    }
}

function Send-OversizedEndpointFrame {
    param([string]$Endpoint, [bool]$WindowsHost)

    [byte[]]$prefix = [BitConverter]::GetBytes([uint32](4 * 1024 * 1024 + 1))
    if ([BitConverter]::IsLittleEndian) {
        [Array]::Reverse($prefix)
    }
    if ($WindowsHost) {
        $pipeName = $Endpoint.Substring("\\.\pipe\".Length)
        $stream = [System.IO.Pipes.NamedPipeClientStream]::new(
            ".",
            $pipeName,
            [System.IO.Pipes.PipeDirection]::InOut,
            [System.IO.Pipes.PipeOptions]::Asynchronous
        )
        try {
            $stream.Connect(5000)
            $stream.Write($prefix, 0, $prefix.Length)
            $stream.Flush()
        }
        finally {
            $stream.Dispose()
        }
        return
    }

    $socket = [System.Net.Sockets.Socket]::new(
        [System.Net.Sockets.AddressFamily]::Unix,
        [System.Net.Sockets.SocketType]::Stream,
        [System.Net.Sockets.ProtocolType]::Unspecified
    )
    try {
        $socket.Connect([System.Net.Sockets.UnixDomainSocketEndPoint]::new($Endpoint))
        if ($socket.Send($prefix) -ne $prefix.Length) {
            throw "failed to send the complete oversized endpoint frame"
        }
    }
    finally {
        $socket.Dispose()
    }
}

try {
if (-not $isWindowsHost) {
    $runtimeBase = if (Test-Path -LiteralPath "/tmp") { "/tmp" } else { [System.IO.Path]::GetTempPath() }
    $endpointRuntimeDir = Join-Path $runtimeBase "cua-$([guid]::NewGuid().ToString('N'))"
    [void][System.IO.Directory]::CreateDirectory($endpointRuntimeDir)
    & chmod 700 $endpointRuntimeDir
    if ($LASTEXITCODE -ne 0) {
        throw "failed to secure the Unix E2E runtime directory"
    }
    $env:XDG_RUNTIME_DIR = $endpointRuntimeDir
}

$help = & $binaryPath --help | Out-String
if ($LASTEXITCODE -ne 0 -or $help -notmatch "host-batch") {
    throw "release CLI help smoke test failed"
}

function Assert-CommandFailureStdoutContract {
    $failureOutput = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-cua-failure-$([guid]::NewGuid().ToString('N')).out"
    $failureError = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-cua-failure-$([guid]::NewGuid().ToString('N')).err"
    $fixtureExtension = if ($isWindowsHost) { ".exe" } else { "" }
    $hostileHost = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-cua-hostile-host-$([guid]::NewGuid().ToString('N'))${fixtureExtension}"
    $failingJsonlHost = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-cua-failing-jsonl-host-$([guid]::NewGuid().ToString('N'))${fixtureExtension}"
    $doctorShutdownFailureHost = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-cua-doctor-shutdown-failure-host-$([guid]::NewGuid().ToString('N'))${fixtureExtension}"
    $hostileHostSource = Join-Path $PSScriptRoot "fixtures\hostile_host.rs"
    $failingJsonlHostSource = Join-Path $PSScriptRoot "fixtures\failing_jsonl_host.rs"
    $doctorShutdownFailureHostSource = Join-Path $PSScriptRoot "fixtures\doctor_shutdown_failure_host.rs"
    $jsonlInput = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-cua-jsonl-failure-$([guid]::NewGuid().ToString('N')).in"
    try {
        & rustc --edition=2024 -o $hostileHost $hostileHostSource
        if ($LASTEXITCODE -ne 0) {
            throw "failed to compile the hostile Host fixture"
        }
        & rustc --edition=2024 -o $failingJsonlHost $failingJsonlHostSource
        if ($LASTEXITCODE -ne 0) {
            throw "failed to compile the failing JSONL Host fixture"
        }
        & rustc --edition=2024 -o $doctorShutdownFailureHost $doctorShutdownFailureHostSource
        if ($LASTEXITCODE -ne 0) {
            throw "failed to compile the doctor shutdown failure Host fixture"
        }

        $failure = Start-Process -FilePath $binaryPath -ArgumentList @("definitely-not-a-command") -RedirectStandardOutput $failureOutput -RedirectStandardError $failureError -PassThru -Wait
        $stdout = [System.IO.File]::ReadAllText($failureOutput, [System.Text.Encoding]::UTF8)
        $stderr = [System.IO.File]::ReadAllText($failureError, [System.Text.Encoding]::UTF8)
        $lines = @($stdout -split "`r?`n" | Where-Object { $_.Length -gt 0 })
        if ($failure.ExitCode -ne 1 -or $lines.Count -ne 1 -or $stderr.Length -ne 0) {
            throw "release CLI failure stream contract was not exit=1/stdout=one-envelope/stderr=empty"
        }
        $envelope = $lines[0] | ConvertFrom-Json
        if ($envelope.success -ne $false -or $envelope.error.code -ne "command_failed") {
            throw "release CLI failure did not return the stable machine envelope"
        }
        if ($envelope.error.message -ne "dcc-cua could not complete the command") {
            throw "release CLI failure did not use the fixed public message"
        }

        $rejectedSyntaxCases = @(
            [pscustomobject]@{ Arguments = @("RELEASE_PRIVATE_ARGUMENT_8e1ab4"); Marker = "RELEASE_PRIVATE_ARGUMENT_8e1ab4" },
            [pscustomobject]@{ Arguments = @("snapshot", "--RELEASE_PRIVATE_OPTION_351cc7"); Marker = "RELEASE_PRIVATE_OPTION_351cc7" }
        )
        foreach ($case in $rejectedSyntaxCases) {
            $rejected = Start-Process -FilePath $binaryPath -ArgumentList $case.Arguments -RedirectStandardOutput $failureOutput -RedirectStandardError $failureError -PassThru -Wait
            $rejectedStdout = [System.IO.File]::ReadAllText($failureOutput, [System.Text.Encoding]::UTF8)
            $rejectedStderr = [System.IO.File]::ReadAllText($failureError, [System.Text.Encoding]::UTF8)
            $rejectedLines = @($rejectedStdout -split "`r?`n" | Where-Object { $_.Length -gt 0 })
            if ($rejected.ExitCode -ne 1 -or $rejectedLines.Count -ne 1 -or $rejectedStderr.Length -ne 0) {
                throw "release CLI rejected syntax did not preserve the failure stream contract"
            }
            $rejectedEnvelope = $rejectedLines[0] | ConvertFrom-Json
            if ($rejectedEnvelope.error.code -ne "command_failed" -or
                $rejectedEnvelope.error.message -ne "dcc-cua could not complete the command" -or
                $rejectedStdout.Contains($case.Marker)) {
                throw "release CLI published rejected command or option text"
            }
        }

        $captured = & $binaryPath definitely-not-a-command 2>$null | Out-String
        $capturedExit = $LASTEXITCODE
        $capturedEnvelope = $captured | ConvertFrom-Json
        if ($capturedExit -ne 1 -or $capturedEnvelope.success -ne $false) {
            throw "command substitution did not receive the failed command envelope from stdout"
        }

        $closedStdoutProbe = @'
import json
import os
import subprocess
import sys

read_fd, write_fd = os.pipe()
os.close(read_fd)
process = None
try:
    process = subprocess.Popen(
        [sys.argv[1], "manifest"],
        stdout=write_fd,
        stderr=subprocess.PIPE,
    )
    os.close(write_fd)
    write_fd = None
    try:
        _, stderr = process.communicate(timeout=10)
        timed_out = False
    except subprocess.TimeoutExpired:
        process.kill()
        _, stderr = process.communicate(timeout=10)
        timed_out = True
    print(json.dumps({
        "exit_code": process.returncode,
        "timed_out": timed_out,
        "stderr_lines": stderr.decode("utf-8").splitlines(),
    }))
finally:
    if write_fd is not None:
        os.close(write_fd)
'@
        $closedStdoutJson = & python -B -c $closedStdoutProbe $binaryPath
        if ($LASTEXITCODE -ne 0) {
            throw "release CLI closed stdout fixture did not complete"
        }
        $closedStdout = $closedStdoutJson | Out-String | ConvertFrom-Json
        $closedStdoutDiagnostics = @($closedStdout.stderr_lines)
        if ($closedStdout.timed_out -or
            $closedStdout.exit_code -ne 1 -or
            $closedStdoutDiagnostics.Count -ne 1 -or
            $closedStdoutDiagnostics[0] -ne "dcc-cua: command result could not be written to stdout") {
            throw "release CLI closed stdout did not emit exactly one fixed safe diagnostic"
        }

        $nativeStart = [System.Diagnostics.ProcessStartInfo]::new($binaryPath)
        $nativeStart.ArgumentList.Add("chrome-extension://abcdefghijklmnop/")
        $nativeStart.UseShellExecute = $false
        $nativeStart.RedirectStandardInput = $true
        $nativeStart.RedirectStandardOutput = $true
        $nativeStart.RedirectStandardError = $true
        $native = [System.Diagnostics.Process]::Start($nativeStart)
        [byte[]]$truncatedPrefix = @(1, 0)
        $native.StandardInput.BaseStream.Write($truncatedPrefix, 0, $truncatedPrefix.Length)
        $native.StandardInput.Close()
        $nativeStdout = $native.StandardOutput.ReadToEnd()
        $nativeStderr = $native.StandardError.ReadToEnd()
        $native.WaitForExit()
        if ($native.ExitCode -ne 1 -or $nativeStdout.Length -ne 0 -or $nativeStderr.Length -ne 0) {
            throw "release Native Messaging failure escaped its framed protocol boundary"
        }

        [System.IO.File]::WriteAllText(
            $jsonlInput,
            "{`"request_id`":`"first`",`"method`":`"ping`",`"params`":{}}`n{`"request_id`":`"second`",`"method`":`"ping`",`"params`":{}}`n",
            [System.Text.UTF8Encoding]::new($false)
        )
        $jsonl = Start-Process -FilePath $binaryPath -ArgumentList @("host-jsonl", "--spawn", $failingJsonlHost) -RedirectStandardInput $jsonlInput -RedirectStandardOutput $failureOutput -RedirectStandardError $failureError -PassThru -Wait
        $jsonlStdout = [System.IO.File]::ReadAllText($failureOutput, [System.Text.Encoding]::UTF8)
        $jsonlStderr = [System.IO.File]::ReadAllText($failureError, [System.Text.Encoding]::UTF8)
        $jsonlResponses = @($jsonlStdout -split "`r?`n" | Where-Object { $_.Length -gt 0 } | ForEach-Object { $_ | ConvertFrom-Json })
        if ($jsonl.ExitCode -ne 1 -or
            $jsonlStderr.Length -ne 0 -or
            $jsonlResponses.Count -ne 2 -or
            $jsonlResponses[0].type -ne "pong" -or
            $jsonlResponses[0].request_id -ne "first" -or
            $jsonlResponses[1].type -ne "error" -or
            $null -ne $jsonlResponses[1].PSObject.Properties["success"]) {
            throw "release host-jsonl mid-stream failure appended a one-shot envelope"
        }

        $hostile = Start-Process -FilePath $binaryPath -ArgumentList @("host-call", "--spawn", $hostileHost, "--method", "ping") -RedirectStandardOutput $failureOutput -RedirectStandardError $failureError -PassThru -Wait
        $hostileStdout = [System.IO.File]::ReadAllText($failureOutput, [System.Text.Encoding]::UTF8)
        $hostileStderr = [System.IO.File]::ReadAllText($failureError, [System.Text.Encoding]::UTF8)
        $hostileLines = @($hostileStdout -split "`r?`n" | Where-Object { $_.Length -gt 0 })
        $hostileEnvelope = $hostileLines[0] | ConvertFrom-Json
        if ($hostile.ExitCode -ne 1 -or
            $hostileLines.Count -ne 1 -or
            $hostileStderr.Length -ne 0 -or
            $hostileEnvelope.error.code -ne "host_protocol_failed" -or
            $hostileEnvelope.error.message -ne "dcc-cua could not complete the command" -or
            $hostileStdout.Contains("CHILD_PRIVATE_DIAGNOSTIC_7e87d1")) {
            throw "spawned Host diagnostics escaped the release CLI structured-output boundary"
        }

        $doctorFailure = Start-Process -FilePath $binaryPath -ArgumentList @("doctor", "--spawn", $doctorShutdownFailureHost) -RedirectStandardOutput $failureOutput -RedirectStandardError $failureError -PassThru -Wait
        $doctorStdout = [System.IO.File]::ReadAllText($failureOutput, [System.Text.Encoding]::UTF8)
        $doctorStderr = [System.IO.File]::ReadAllText($failureError, [System.Text.Encoding]::UTF8)
        $doctorDocument = $null
        $oneShotSuccess = [System.Text.Json.JsonElement]::new()
        try {
            $doctorDocument = [System.Text.Json.JsonDocument]::Parse($doctorStdout)
            $doctorRoot = $doctorDocument.RootElement
            $doctorType = $doctorRoot.GetProperty("type").GetString()
            $doctorReady = $doctorRoot.GetProperty("ready").GetBoolean()
            $hasOneShotSuccess = $doctorRoot.TryGetProperty("success", [ref]$oneShotSuccess)
        }
        catch {
            throw "release Host doctor did not publish exactly one diagnostics document: $_"
        }
        finally {
            if ($null -ne $doctorDocument) {
                $doctorDocument.Dispose()
            }
        }
        if ($doctorFailure.ExitCode -ne 1 -or
            $doctorStderr.Length -ne 0 -or
            $doctorType -ne "diagnostics" -or
            $doctorReady -ne $false -or
            $hasOneShotSuccess) {
            throw "release Host doctor shutdown failure violated its diagnostics-native boundary"
        }
    }
    finally {
        foreach ($path in @($failureOutput, $failureError, $hostileHost, $failingJsonlHost, $doctorShutdownFailureHost, $jsonlInput)) {
            if (Test-Path -LiteralPath $path) {
                Remove-Item -LiteralPath $path -Force
            }
        }
    }
}

Assert-CommandFailureStdoutContract

function Assert-OneShotTtyStreamContract {
    if ($isWindowsHost -or $isMacHost) {
        return
    }
    if ($null -eq (Get-Command script -ErrorAction SilentlyContinue)) {
        throw "the Linux release CLI TTY contract requires util-linux script"
    }

    $ttyRoot = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-cua-tty-$([guid]::NewGuid().ToString('N'))"
    $cacheDirectory = Join-Path $ttyRoot ".dcc-cua/cache"
    [void][System.IO.Directory]::CreateDirectory($cacheDirectory)
    [System.IO.File]::WriteAllText(
        (Join-Path $cacheDirectory "update-check.json"),
        "{`n  `"next_check_unix_secs`": 4102444800,`n  `"latest_version`": `"999.0.0`"`n}`n",
        [System.Text.UTF8Encoding]::new($false)
    )

    function ConvertTo-ShellLiteral([string]$Value) {
        return "'" + $Value.Replace("'", "'`"'`"'") + "'"
    }

    try {
        $quotedBinary = ConvertTo-ShellLiteral $binaryPath
        $quotedHome = ConvertTo-ShellLiteral $ttyRoot
        $cases = @(
            [pscustomobject]@{ Name = "stdout-file"; Redirect = "> {stdout}"; StdoutFile = $true; StderrFile = $false },
            [pscustomobject]@{ Name = "stderr-file"; Redirect = "2> {stderr}"; StdoutFile = $false; StderrFile = $true },
            [pscustomobject]@{ Name = "stdin-stdout-files"; Redirect = "< /dev/null > {stdout}"; StdoutFile = $true; StderrFile = $false },
            [pscustomobject]@{ Name = "all-files"; Redirect = "< /dev/null > {stdout} 2> {stderr}"; StdoutFile = $true; StderrFile = $true }
        )
        foreach ($case in $cases) {
            $stdoutPath = Join-Path $ttyRoot "$($case.Name).out"
            $stderrPath = Join-Path $ttyRoot "$($case.Name).err"
            $transcriptPath = Join-Path $ttyRoot "$($case.Name).tty"
            $redirect = $case.Redirect.Replace("{stdout}", (ConvertTo-ShellLiteral $stdoutPath)).Replace("{stderr}", (ConvertTo-ShellLiteral $stderrPath))
            $command = "HOME=$quotedHome USERPROFILE=$quotedHome CI= $quotedBinary manifest $redirect"
            & script -q -e -c $command $transcriptPath *> $null
            if ($LASTEXITCODE -ne 0) {
                throw "release CLI TTY matrix case '$($case.Name)' failed with exit $LASTEXITCODE"
            }
            $ttyText = [System.IO.File]::ReadAllText($transcriptPath, [System.Text.Encoding]::UTF8)
            $stderrText = if (Test-Path -LiteralPath $stderrPath) { [System.IO.File]::ReadAllText($stderrPath, [System.Text.Encoding]::UTF8) } else { "" }
            if ($ttyText.Contains("999.0.0") -or $ttyText.Contains("A new version of dcc-cua") -or
                $stderrText.Contains("999.0.0") -or $stderrText.Contains("A new version of dcc-cua")) {
                throw "release CLI TTY matrix case '$($case.Name)' leaked a dynamic update notice"
            }
            if ($case.StderrFile -and $stderrText.Length -ne 0) {
                throw "release CLI TTY matrix case '$($case.Name)' wrote non-diagnostic stderr"
            }
            if ($case.StdoutFile) {
                $json = [System.IO.File]::ReadAllText($stdoutPath, [System.Text.Encoding]::UTF8) | ConvertFrom-Json
                if ($json.name -ne "dcc-cua") {
                    throw "release CLI TTY matrix case '$($case.Name)' did not preserve manifest stdout"
                }
            }
        }
    }
    finally {
        if (Test-Path -LiteralPath $ttyRoot) {
            Remove-Item -LiteralPath $ttyRoot -Recurse -Force
        }
    }
}

Assert-OneShotTtyStreamContract

$manifest = Invoke-BinaryJson -Arguments @("manifest")
$expectedOs = if ($isWindowsHost) { "windows" } elseif ($isMacHost) { "macos" } else { "linux" }
if ($manifest.name -ne "dcc-cua" -or $manifest.target.os -ne $expectedOs) {
    throw "manifest does not describe the current release binary"
}
if ($manifest.runtime.backend -ne "cua-driver-sdk" -or $manifest.runtime.separate_driver_required -ne $false) {
    throw "manifest still requires a separate CUA driver"
}
if (-not $isWindowsHost -and
    $manifest.host.default_endpoint -ne (Join-Path $endpointRuntimeDir "dcc-cua-v1.sock")) {
    throw "manifest did not select the private XDG runtime endpoint"
}
if ($manifest.version -notmatch '^\d+\.\d+\.\d+$' -or
    $manifest.host.protocol_version -ne 1 -or
    $manifest.host.max_connections -ne 32 -or
    $manifest.host.hello_timeout_ms -ne 10000 -or
    $manifest.host.max_parallel_discovery_requests -ne 32) {
    throw "manifest version or Host protocol is invalid"
}
if ($manifest.host.grant_limits.task_grant_id_max_chars -ne 128 -or
    $manifest.host.grant_limits.application_label_max_chars -ne 80) {
    throw "manifest omitted the Host grant limits"
}
if ($manifest.host.snapshot_transports -notcontains "shared_memory" -or
    $manifest.host.capabilities -notcontains "two_axis_scroll" -or
    $manifest.host.capabilities -notcontains "host_ping" -or
    $manifest.host.capabilities -notcontains "host_diagnostics" -or
    $manifest.host.capabilities -notcontains "session_scoped_application_lifecycle") {
    throw "manifest omitted required Host capabilities"
}

$profileStore = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-cua-profile-$([guid]::NewGuid().ToString('N'))"
$profilePackageSource = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..\examples\profiles\the-bazaar")).Path
$profilePackageCopy = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-cua-profile-source-$([guid]::NewGuid().ToString('N'))"
Copy-Item -LiteralPath $profilePackageSource -Destination $profilePackageCopy -Recurse
$profileManifestPath = Join-Path $profilePackageCopy "profile-package.json"
$profileManifest = Get-Content -LiteralPath $profileManifestPath -Raw | ConvertFrom-Json
$profileManifest.platforms = @($expectedOs)
[System.IO.File]::WriteAllText(
    $profileManifestPath,
    ($profileManifest | ConvertTo-Json -Depth 10),
    [System.Text.UTF8Encoding]::new($false)
)
$profilePackage = $profilePackageCopy
$profileValidation = Invoke-BinaryJson -Arguments @(
    "profile", "validate", $profilePackage,
    "--profile-store", $profileStore
)
if ($profileValidation.id -ne "the-bazaar" -or
    $profileValidation.requires.dcc_cua -ne ">=0.6.0" -or
    @($profileValidation.artifacts | Where-Object { $_.type -eq "context_index" }).Count -ne 1) {
    throw "The Bazaar profile package did not validate its typed context artifacts"
}
$profileInstallation = Invoke-BinaryJson -Arguments @(
    "profile", "install", $profilePackage,
    "--profile-store", $profileStore
)
if ($profileInstallation.id -ne "the-bazaar") {
    throw "The Bazaar profile package installation failed"
}
$profileContext = Invoke-BinaryJson -Arguments @(
    "profile", "context",
    "--id", "the-bazaar",
    "--identity", "game-data=sha256:e2e-unmatched-data",
    "--selector", "character=Pygmalien",
    "--profile-store", $profileStore
)
if ($profileContext.profileId -ne "the-bazaar" -or
    $profileContext.schemaVersion -ne 2 -or
    $profileContext.selection -ne "exact" -or
    @($profileContext.documents).Count -ne 1 -or
    $profileContext.documents[0].id -ne "base-rules") {
    throw "profile context did not load the generic unfenced base document"
}

$batchRequest = @(
    [ordered]@{ request_id = "e2e-ping"; method = "ping"; params = @{} },
    [ordered]@{ request_id = "e2e-apps"; method = "list_apps"; params = @{} },
    [ordered]@{ request_id = "e2e-tools"; method = "list_tools"; params = @{} }
) | ConvertTo-Json -Depth 4 -Compress
$batchFile = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-cua-e2e-$([guid]::NewGuid().ToString('N')).json"
[System.IO.File]::WriteAllText($batchFile, $batchRequest, [System.Text.UTF8Encoding]::new($false))
try {
    $responses = @(Invoke-BinaryJson -Arguments @(
        "host-batch",
        "--spawn", $binaryPath,
        "--snapshot-transport", "shared_memory",
        "--json-file", $batchFile
    ))
}
finally {
    if (Test-Path -LiteralPath $batchFile) {
        Remove-Item -LiteralPath $batchFile -Force
    }
}

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

$streamArguments = @(
    "host-jsonl",
    "--spawn", $binaryPath,
    "--parallel-discovery",
    "--snapshot-transport", "shared_memory"
)
$streamRequests = @(
    '{"request_id":"stream-ping-1","method":"ping","params":{}}',
    '{"request_id":"stream-doctor","method":"doctor","params":{}}',
    '{"request_id":"stream-desktop-open","method":"open_desktop_session","params":{"session_id":"cli-e2e-lifecycle","grant":{"task_grant_id":"cli-e2e","application_label":"Desktop","allow_raw_input":false}}}',
    '{"request_id":"stream-desktop-stop","method":"stop_desktop_session","params":{"session_id":"cli-e2e-lifecycle"}}',
    '{"request_id":"stream-error","method":"unknown_method","params":{}}',
    '{"request_id":"stream-ping-2","method":"ping","params":{}}'
)
$streamBurstCount = 2 * [int]$manifest.host.max_parallel_discovery_requests
for ($index = 0; $index -lt $streamBurstCount; $index++) {
    $streamRequests += "{`"request_id`":`"stream-burst-$index`",`"method`":`"ping`",`"params`":{}}"
}
$streamInput = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-cua-e2e-$([guid]::NewGuid().ToString('N')).jsonl"
$streamOutput = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-cua-e2e-$([guid]::NewGuid().ToString('N')).out"
$streamError = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-cua-e2e-$([guid]::NewGuid().ToString('N')).err"
[System.IO.File]::WriteAllLines($streamInput, $streamRequests, [System.Text.UTF8Encoding]::new($true))
try {
    $stream = Start-Process -FilePath $binaryPath -ArgumentList $streamArguments -RedirectStandardInput $streamInput -RedirectStandardOutput $streamOutput -RedirectStandardError $streamError -PassThru -Wait
    if ($stream.ExitCode -ne 0) {
        throw "host-jsonl failed: $([System.IO.File]::ReadAllText($streamError))"
    }
    $streamResponses = @(Get-Content -LiteralPath $streamOutput | ForEach-Object { $_ | ConvertFrom-Json })
}
finally {
    foreach ($path in @($streamInput, $streamOutput, $streamError)) {
        if (Test-Path -LiteralPath $path) {
            Remove-Item -LiteralPath $path -Force
        }
    }
}

$streamJson = $streamResponses | ConvertTo-Json -Depth 12 -Compress
if ($streamResponses[0].request_id -ne "stream-ping-1" -or
    $streamResponses[0].type -ne "pong" -or
    $streamResponses[1].request_id -ne "stream-doctor" -or
    $streamResponses[1].type -ne "diagnostics" -or
    $streamResponses[1].schema_version -ne 1 -or
    $null -eq $streamResponses[1].checks.driver.success -or
    $null -eq $streamResponses[1].checks.health.success -or
    $null -eq $streamResponses[1].checks.interactive_desktop.success -or
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
elseif ($streamResponses[1].ready -ne $false -or
    $streamResponses[2].request_id -ne "stream-desktop-open" -or
    $streamResponses[2].type -ne "error" -or
    [string]::IsNullOrWhiteSpace($streamResponses[2].code) -or
    $streamResponses[3].request_id -ne "stream-desktop-stop" -or
    $streamResponses[3].type -ne "error" -or
    [string]::IsNullOrWhiteSpace($streamResponses[3].code)) {
    throw "Host session lifecycle was not available without a structured readiness refusal: $streamJson"
}
if ($isWindowsHost -and
    $streamResponses[1].checks.interactive_desktop.code -eq "interactive_session_not_active" -and
    $streamResponses[2].code -ne "interactive_desktop_unavailable") {
    throw "disconnected Windows Host did not return the dedicated desktop-readiness error: $streamJson"
}
if ($streamResponses.Count -ne (6 + $streamBurstCount)) {
    throw "long-lived Host stream dropped bounded discovery responses"
}
for ($index = 0; $index -lt $streamBurstCount; $index++) {
    $response = $streamResponses[$index + 6]
    if ($response.request_id -ne "stream-burst-$index" -or $response.type -ne "pong") {
        throw "long-lived Host stream lost bounded discovery response $index"
    }
}

$endpointHost = $null
$endpointHostStartTime = $null
$endpoint = if ($isWindowsHost) {
    "\\.\pipe\dcc-cua-e2e-$([guid]::NewGuid().ToString('N'))"
} else {
    [string]$manifest.host.default_endpoint
}
$endpointPingCount = [int]$manifest.host.max_parallel_discovery_requests - 2
$endpointBatchRequests = @(
    [ordered]@{ request_id = "endpoint-apps"; method = "list_apps"; params = @{} },
    [ordered]@{ request_id = "endpoint-tools"; method = "list_tools"; params = @{} }
)
for ($index = 0; $index -lt $endpointPingCount; $index++) {
    $endpointBatchRequests += [ordered]@{
        request_id = "endpoint-ping-$index"
        method = "ping"
        params = @{}
    }
}
$endpointBatchJson = $endpointBatchRequests | ConvertTo-Json -Depth 4 -Compress
$endpointBatchFile = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-cua-e2e-$([guid]::NewGuid().ToString('N')).json"
[System.IO.File]::WriteAllText($endpointBatchFile, $endpointBatchJson, [System.Text.UTF8Encoding]::new($false))
try {
    $ensured = Invoke-BinaryJson -Arguments @("host-ensure", "--endpoint", $endpoint)
    if ($ensured.type -ne "host_ready" -or
        $ensured.status -ne "started" -or
        [string]::IsNullOrWhiteSpace([string]$ensured.pid)) {
        throw "host-ensure did not start the endpoint Host"
    }
    $startedHost = [System.Diagnostics.Process]::GetProcessById([int]$ensured.pid)
    $startedHostStartTime = $startedHost.StartTime.ToUniversalTime().Ticks
    $pathComparison = if ($isWindowsHost) {
        [System.StringComparison]::OrdinalIgnoreCase
    } else {
        [System.StringComparison]::Ordinal
    }
    $endpointHostPath = [System.IO.Path]::GetFullPath($startedHost.MainModule.FileName)
    if (-not [string]::Equals($endpointHostPath, $binaryPath, $pathComparison)) {
        $startedHost.Dispose()
        throw "host-ensure returned an unexpected process: $endpointHostPath"
    }
    $endpointHost = $startedHost
    $endpointHostStartTime = $startedHostStartTime
    $ensuredAgain = Invoke-BinaryJson -Arguments @("host-ensure", "--endpoint", $endpoint)
    if ($ensuredAgain.type -ne "host_ready" -or
        $ensuredAgain.status -ne "existing" -or
        $ensuredAgain.endpoint -ne $endpoint) {
        throw "host-ensure is not idempotent"
    }

    Send-OversizedEndpointFrame -Endpoint $endpoint -WindowsHost $isWindowsHost
    Start-Sleep -Milliseconds 100
    $recoveryPing = & $binaryPath host-call --endpoint $endpoint --method ping |
        Out-String | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $recoveryPing.type -ne "pong") {
        throw "cross-platform Host endpoint did not recover after a malformed frame"
    }

    $endpointInterrupt = & $binaryPath interrupt-all --endpoint $endpoint |
        Out-String | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or
        $endpointInterrupt.type -ne "interrupt_broadcast" -or
        $endpointInterrupt.scope -ne "host_process") {
        throw "cross-platform Host endpoint did not accept a shared stop"
    }

    $endpointBatch = & $binaryPath host-batch --endpoint $endpoint --json-file $endpointBatchFile |
        Out-String | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or
        $endpointBatch.Count -ne (2 + $endpointPingCount) -or
        $endpointBatch[0].request_id -ne "endpoint-apps" -or
        $endpointBatch[0].type -ne "apps" -or
        $endpointBatch[1].request_id -ne "endpoint-tools" -or
        $endpointBatch[1].type -ne "tools") {
        throw "cross-platform Host endpoint batch IPC failed"
    }
    for ($index = 0; $index -lt $endpointPingCount; $index++) {
        $response = $endpointBatch[$index + 2]
        if ($response.request_id -ne "endpoint-ping-$index" -or $response.type -ne "pong") {
            throw "bounded parallel Host endpoint burst lost response $index"
        }
    }
}
finally {
    if ($null -ne $endpointHost) {
        try {
            $endpointHost.Refresh()
            if (-not $endpointHost.HasExited) {
                if ($endpointHost.StartTime.ToUniversalTime().Ticks -ne $endpointHostStartTime) {
                    throw "refusing to stop a reused Host process identifier"
                }
                $endpointHost.Kill()
                [void]$endpointHost.WaitForExit(5000)
            }
            $endpointHost.Dispose()
        }
        catch [System.ArgumentException] {}
    }
    if (-not $isWindowsHost -and (Test-Path -LiteralPath $endpoint)) {
        Remove-Item -LiteralPath $endpoint -Force
    }
    if (Test-Path -LiteralPath $endpointBatchFile) {
        Remove-Item -LiteralPath $endpointBatchFile -Force
    }
}

Write-Host "CLI E2E passed for ${expectedOs}: self-contained SDK runtime, manifest, diagnostics, session lifecycle, idempotent Host ensure, a ${streamBurstCount}-request long-lived discovery burst, bounded endpoint batch/stream Host IPC, error recovery, apps, and $($toolNames.Count) CUA tools."
}
finally {
    if ($null -ne $profileStore -and (Test-Path -LiteralPath $profileStore)) {
        Remove-Item -LiteralPath $profileStore -Recurse -Force
    }
    if ($null -ne $profilePackageCopy -and (Test-Path -LiteralPath $profilePackageCopy)) {
        Remove-Item -LiteralPath $profilePackageCopy -Recurse -Force
    }
    if (-not $isWindowsHost) {
        if ($null -eq $originalXdgRuntimeDir) {
            Remove-Item Env:XDG_RUNTIME_DIR -ErrorAction SilentlyContinue
        }
        else {
            $env:XDG_RUNTIME_DIR = $originalXdgRuntimeDir
        }
        if ($null -ne $endpointRuntimeDir) {
            $runtimeSocket = Join-Path $endpointRuntimeDir "dcc-cua-v1.sock"
            if (Test-Path -LiteralPath $runtimeSocket) {
                Remove-Item -LiteralPath $runtimeSocket -Force
            }
            if (Test-Path -LiteralPath $endpointRuntimeDir) {
                Remove-Item -LiteralPath $endpointRuntimeDir -Force
            }
        }
    }
}
