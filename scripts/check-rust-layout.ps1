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
        $sourceLines = @(Get-Content -LiteralPath $path)
        if ($sourceLines.Count -gt $maxLines) {
            $violations.Add("$relative has $($sourceLines.Count) lines; maximum is $maxLines")
        }

        $isTestFile = $path -match "[\\/]src[\\/]tests\.rs$" -or $path -match "[\\/]tests[\\/]"
        if ($isTestFile) {
            if ($content -notmatch "use rstest::rstest;") {
                $violations.Add("$relative must import rstest")
            }
            for ($lineIndex = 0; $lineIndex -lt $sourceLines.Count; $lineIndex++) {
                $attribute = $sourceLines[$lineIndex].Trim()
                if ($attribute -eq "#[test]") {
                    $violations.Add("$relative`:$($lineIndex + 1) must use rstest instead of #[test]")
                }
                if ($attribute -match '^#\[tokio::test(?:\([^]]*\))?\]$' -and
                    ($lineIndex -eq 0 -or $sourceLines[$lineIndex - 1].Trim() -ne "#[rstest]")) {
                    $violations.Add("$relative`:$($lineIndex + 1) must place #[rstest] directly before #[tokio::test]")
                }
            }
        } else {
            $inlineTest = $sourceLines |
                Where-Object { $_.Trim() -match '^#\[(?:test|rstest|tokio::test(?:\([^]]*\))?)\]$' } |
                Select-Object -First 1
            if ($null -ne $inlineTest) {
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
