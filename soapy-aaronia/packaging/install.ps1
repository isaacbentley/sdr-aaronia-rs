# Install the Aaronia SoapySDR module next to this script.
#
# Finds where SoapySDR loads modules from and copies the plugin there.
# Prints manual instructions rather than guessing when it cannot tell.
$ErrorActionPreference = 'Stop'

$dir = Split-Path -Parent $MyInvocation.MyCommand.Path
$module = Join-Path $dir 'aaroniaSupport.dll'

if (-not (Test-Path $module)) {
    Write-Error "No plugin found next to this script. Run it from the unpacked release archive."
    exit 1
}

$util = Get-Command SoapySDRUtil -ErrorAction SilentlyContinue
if (-not $util) {
    Write-Host "SoapySDRUtil is not on PATH, so the module directory cannot be found."
    Write-Host ""
    Write-Host "Install SoapySDR, or use the plugin without installing:"
    Write-Host "    `$env:SOAPY_SDR_PLUGIN_PATH = '$dir'"
    Write-Host "    SoapySDRUtil --check=aaronia"
    exit 1
}

$info = & SoapySDRUtil --info 2>$null

$target = ($info |
    Select-String -Pattern '^Search path:\s*(.+)$' |
    ForEach-Object { $_.Matches[0].Groups[1].Value.Trim() } |
    Select-Object -First 1)

if (-not $target) {
    # Fall back to the directory of an already-installed module, the
    # same fallback install.sh uses. SoapySDRUtil appends " (version)"
    # to each module line.
    $found = ($info |
        Select-String -Pattern '^Module found:\s*(.+?)(\s+\([^)]*\))?$' |
        ForEach-Object { $_.Matches[0].Groups[1].Value.Trim() } |
        Select-Object -First 1)
    if ($found) { $target = Split-Path -Parent $found }
}

if (-not $target -or -not (Test-Path $target)) {
    Write-Host "Could not determine SoapySDR's module directory from SoapySDRUtil --info."
    Write-Host ""
    Write-Host "Use the plugin without installing:"
    Write-Host "    `$env:SOAPY_SDR_PLUGIN_PATH = '$dir'"
    Write-Host "    SoapySDRUtil --check=aaronia"
    exit 1
}

Write-Host "Installing aaroniaSupport.dll into $target"
Copy-Item $module $target -Force

Write-Host ""
Write-Host "Checking the module loads:"
& SoapySDRUtil --check=aaronia
Write-Host ""
Write-Host "Done. Try your device with:"
Write-Host '    SoapySDRUtil --probe="driver=aaronia,url=http://localhost:54664"'
