$ErrorActionPreference = "Stop"

$maxLines = 2000
$violations = [System.Collections.Generic.List[string]]::new()
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

Get-ChildItem -Path (Join-Path $PSScriptRoot "..\crates") -Recurse -Filter *.rs |
    Where-Object { $_.FullName -notmatch "[\\/]target[\\/]" } |
    ForEach-Object {
        $path = $_.FullName
        $relative = $path.Substring($repoRoot.Length + 1)
        $content = Get-Content -LiteralPath $path -Raw
        $lines = (Get-Content -LiteralPath $path).Count
        if ($lines -gt $maxLines) {
            $violations.Add("$relative has $lines lines; maximum is $maxLines")
        }

        $isTestFile = $path -match "[\\/]src[\\/]tests\.rs$" -or $path -match "[\\/]tests[\\/]"
        if ($isTestFile) {
            if ($content -notmatch "use rstest::rstest;") {
                $violations.Add("$relative must import rstest")
            }
            if ($content -match "(?m)^\s*#\[test\]\s*$") {
                $violations.Add("$relative must use rstest instead of #[test]")
            }
            if ($content -match "(?m)^\s*#\[tokio::test\]\s*$" -and
                $content -notmatch "(?ms)#\[rstest\]\s*\r?\n\s*#\[tokio::test\]") {
                $violations.Add("$relative async tests must combine rstest with tokio::test")
            }
        } else {
            if ($content -match "(?m)^\s*#\[(?:test|rstest|tokio::test)\]\s*$") {
                $violations.Add("$relative contains an inline test; move it to src/tests.rs")
            }
            if ($content -match "(?m)^\s*#\[cfg\(test\)\]\s*mod tests\s*\{") {
                $violations.Add("$relative contains an inline test module; move it to src/tests.rs")
            }
            $production = [regex]::Replace(
                $content,
                '(?m)^\s*#\[cfg\(test\)\]\s*\r?\n\s*mod tests;\s*$',
                ''
            )
            if ($production -match "#\[cfg\(test\)\]") {
                $violations.Add("$relative contains test-only code outside its separated test module")
            }
        }
    }

if ($violations.Count -gt 0) {
    $violations | ForEach-Object { Write-Error $_ }
    exit 1
}

Write-Output "Rust layout gate passed: every Rust file is <= $maxLines lines and tests are separated under src/tests.rs using rstest."
