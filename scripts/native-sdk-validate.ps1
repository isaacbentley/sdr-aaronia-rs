<#
.SYNOPSIS
  Exhaustive native-SDK validation against an attached Spectran V6.

.DESCRIPTION
  Runs the whole native-SDK matrix on a Windows machine with a device:
  detection, ABI/symbol resolution, the hardware suite, and the two
  cross-API paths (Seify, C ABI) that the default feature set leaves
  untested.

  IMPORTANT: RTSA-Suite PRO must be CLOSED. The suite's HTTP Server
  block holds the device exclusively; the native SDK cannot open it at
  the same time, and the failure surfaces as "No Spectran V6 devices
  found" rather than as a busy device.

.EXAMPLE
  .\scripts\native-sdk-validate.ps1
  .\scripts\native-sdk-validate.ps1 -Trials 10 -SoakSeconds 300
#>
param(
    [int]    $Trials      = 5,
    [double] $SoakSeconds = 60,
    [double] $CenterHz    = 2.44e9,
    [string] $Serial      = "",
    [string] $LogDir      = "validation-logs"
)

$ErrorActionPreference = 'Continue'
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$log   = Join-Path $LogDir "native-sdk-$stamp.log"

function Section($name) {
    $line = "=" * 70
    Write-Host ""
    Write-Host $line -ForegroundColor Cyan
    Write-Host "  $name" -ForegroundColor Cyan
    Write-Host $line -ForegroundColor Cyan
    "`n$line`n  $name`n$line" | Out-File -Append $log
}

function Run($label, $cmd) {
    Write-Host "`n--- $label ---" -ForegroundColor Yellow
    "`n--- $label ---`n$cmd" | Out-File -Append $log
    $out = Invoke-Expression "$cmd 2>&1" | Out-String
    $out | Out-File -Append $log
    Write-Host $out
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FAILED ($LASTEXITCODE): $label" -ForegroundColor Red
        return $false
    }
    return $true
}

$env:AARONIA_SDK_TRIALS     = $Trials
$env:AARONIA_SDK_SOAK_SECS  = $SoakSeconds
$env:AARONIA_SDK_CENTER     = $CenterHz
if ($Serial) { $env:AARONIA_SDK_SERIAL = $Serial }

$results = [ordered]@{}

Section "0. Preflight"
Write-Host "repo   : $repo"
Write-Host "log    : $log"
Write-Host "rustc  : $(rustc --version)"
$sdkDefault = "C:\Program Files\Aaronia AG\Aaronia RTSA-Suite PRO"
if ($env:AARONIA_SDK_PATH) {
    Write-Host "SDK    : $env:AARONIA_SDK_PATH (from AARONIA_SDK_PATH)"
} elseif (Test-Path $sdkDefault) {
    Write-Host "SDK    : $sdkDefault (default install)"
} else {
    Write-Host "SDK    : NOT FOUND — set AARONIA_SDK_PATH" -ForegroundColor Red
}
$rtsa = Get-Process -Name "*RTSA*" -ErrorAction SilentlyContinue
if ($rtsa) {
    Write-Host ""
    Write-Host "RTSA-Suite PRO is RUNNING and holds the device exclusively." -ForegroundColor Red
    Write-Host "Close it before continuing, or every open below fails as" -ForegroundColor Red
    Write-Host "'No Spectran V6 devices found'." -ForegroundColor Red
    Write-Host ""
    $reply = Read-Host "Continue anyway? (y/N)"
    if ($reply -ne 'y') { exit 1 }
}

Section "1. Build"
$results['build'] = Run "cargo build --features native-sdk" `
    "cargo build --features native-sdk"

Section "2. Non-hardware suite (must be green before hardware means anything)"
$results['unit'] = Run "cargo test --features native-sdk" `
    "cargo test --features native-sdk"

Section "3. Library load and ABI symbol resolution (no device needed)"
$results['abi'] = Run "native_sdk_load" `
    "cargo test --features native-sdk --test native_sdk_load -- --ignored --nocapture"

Section "4. Hardware suite"
# --test-threads=1 is belt-and-braces: the suite also takes an internal
# lock, because the device admits exactly one holder.
$results['live'] = Run "native_sdk_live" `
    "cargo test --features native-sdk --test native_sdk_live -- --ignored --nocapture --test-threads=1"

Section "5. Cross-API: Seify over the native SDK"
$results['seify'] = Run "seify + native-sdk" `
    "cargo test --features seify,native-sdk --test native_sdk_live -- --ignored --nocapture --test-threads=1 seify_"

Section "6. Cross-API: C ABI over the native SDK"
$results['c_abi'] = Run "ffi + native-sdk" `
    "cargo test --features ffi,native-sdk --test native_sdk_live -- --ignored --nocapture --test-threads=1 c_api_"

Section "Summary"
$failed = 0
foreach ($k in $results.Keys) {
    if ($results[$k]) {
        Write-Host ("  {0,-8} PASS" -f $k) -ForegroundColor Green
        ("  {0,-8} PASS" -f $k) | Out-File -Append $log
    } else {
        Write-Host ("  {0,-8} FAIL" -f $k) -ForegroundColor Red
        ("  {0,-8} FAIL" -f $k) | Out-File -Append $log
        $failed++
    }
}
Write-Host ""
Write-Host "Full log: $log"
exit $failed
