param(
    [Parameter(Mandatory = $true)][string]$NsisPath,
    [string]$OutputRoot = 'D:/BuildCache/hagency-client/updater-handoff',
    [string]$Toolchain = '1.98.0'
)
$ErrorActionPreference = 'Stop'
if (-not [System.IO.Path]::GetFullPath($OutputRoot).StartsWith('D:\', [System.StringComparison]::OrdinalIgnoreCase)) {
    throw 'The Windows updater fixture must use D:/BuildCache'
}
$repo = Split-Path $PSScriptRoot -Parent
$previousKey = $env:HAGENCY_UPDATER_PUBLIC_KEY
$previousData = $env:HAGENCY_APP_DATA_DIR
$previousMarker = $env:HAGENCY_UPDATE_TEST_MARKER
$previousToolchain = $env:RUSTUP_TOOLCHAIN
$previousBusy = $env:HAGENCY_UPDATE_TEST_BUSY
$previousTemp = $env:TEMP
$previousTmp = $env:TMP
$env:RUSTUP_TOOLCHAIN = $Toolchain
$testRoot = Join-Path $OutputRoot ([guid]::NewGuid().ToString())
New-Item -ItemType Directory -Force -Path $testRoot | Out-Null
$env:CARGO_HOME = 'D:/BuildCache/hagency-client/cargo'
$env:CARGO_TARGET_DIR = 'D:/BuildCache/hagency-client/target'
$temporary = Join-Path $testRoot 'tmp'
New-Item -ItemType Directory -Force -Path $temporary | Out-Null
$env:TEMP = $temporary
$env:TMP = $temporary
Push-Location $repo
try {
    $metadata = (& cargo metadata --no-deps --locked --format-version 1 | ConvertFrom-Json)
    if ([System.IO.Path]::GetFullPath($metadata.target_directory) -ne [System.IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)) { throw 'Cargo target directory changed' }
    $probeSource = Join-Path $testRoot 'probe.rs'
    @'
fn main() {
    let path = std::env::var_os("HAGENCY_UPDATE_TEST_MARKER").expect("fixture marker");
    std::fs::write(path, "new version started").unwrap();
}
'@ | Set-Content -LiteralPath $probeSource -Encoding utf8
    $probe = Join-Path $testRoot 'new-version.exe'
    & rustc $probeSource -o $probe
    if ($LASTEXITCODE -ne 0) { throw 'Could not compile restart probe' }
    # A refused installer restarts the existing executable. Use a real PE here:
    # a text placeholder makes CreateProcess show an unsupported-application dialog.
    $oldProbeSource = Join-Path $testRoot 'old_probe.rs'
    @'
fn main() {
    let path = std::path::PathBuf::from(std::env::var_os("HAGENCY_UPDATE_TEST_MARKER").expect("fixture marker"));
    std::fs::write(path.with_extension("old"), "old version started").unwrap();
}
'@ | Set-Content -LiteralPath $oldProbeSource -Encoding utf8
    $oldProbe = Join-Path $testRoot 'old-version.exe'
    & rustc $oldProbeSource -o $oldProbe
    if ($LASTEXITCODE -ne 0) { throw 'Could not compile old-version restart probe' }
    $installer = Join-Path $testRoot 'fixture-installer.exe'
    $recipe = Join-Path $testRoot 'fixture.nsi'
    $app = $metadata.packages | Where-Object name -eq 'hagency-app'
    $template = Join-Path (Split-Path $app.manifest_path -Parent) $app.metadata.packager.nsis.template
    $templateText = Get-Content -LiteralPath $template -Raw
    $processMacro = [regex]::Match($templateText, '(?s)!macro CheckIfAppIsRunning\b.*?!macroend').Value
    if (-not $processMacro -or $processMacro.Contains('KillProcess')) { throw 'Installer must refuse other instances without terminating them' }
    # Exercise the exact production refusal macro with a deterministic process
    # probe. No plugin or real user process is needed for the busy branch.
    $processMacro = $processMacro.Replace('nsis_tauri_utils::FindProcess "${MAINBINARYNAME}.exe"', 'ReadEnvStr $R0 "HAGENCY_UPDATE_TEST_BUSY"' + "`n  Push `$R0")
    $processMacro = $processMacro.Replace('$(appRunning)', 'Another Hagency instance is running')
    @"
Unicode true
!include LogicLib.nsh
Name "Hagency offline update fixture"
OutFile "$installer"
RequestExecutionLevel user
SilentInstall silent
$processMacro
Section
!insertmacro CheckIfAppIsRunning
SetOutPath `$INSTDIR
File /oname=hagency.exe "$probe"
SectionEnd
"@ | Set-Content -LiteralPath $recipe -Encoding utf8
    & $NsisPath /V2 $recipe
    if ($LASTEXITCODE -ne 0) { throw 'Could not compile NSIS fixture' }
    $fixture = Join-Path $testRoot 'fixture'
    & cargo run --locked -p hagency-updater --example stage_fixture -- $fixture $installer
    if ($LASTEXITCODE -ne 0) { throw 'Could not sign offline fixture' }
    $env:HAGENCY_UPDATER_PUBLIC_KEY = Get-Content -LiteralPath (Join-Path $testRoot 'fixture-public-key.txt') -Raw
    & cargo build --locked -p hagency-app --bin hagency --features agent_chat,octosense-module
    if ($LASTEXITCODE -ne 0) { throw 'Could not build update helper' }
    $binary = Join-Path $env:CARGO_TARGET_DIR 'debug/hagency.exe'
    $results = @()
    foreach ($scenario in @('install', 'cancel', 'tamper', 'busy')) {
        $env:HAGENCY_UPDATE_TEST_BUSY = if ($scenario -eq 'busy') { '0' } else { '1' }
        $data = Join-Path $testRoot $scenario
        Copy-Item -LiteralPath $fixture -Destination $data -Recurse
        $updates = Join-Path $data 'updates'
        $helperDir = Join-Path $updates ('apply-' + [guid]::NewGuid().ToString())
        New-Item -ItemType Directory -Path $helperDir | Out-Null
        $helperExe = Join-Path $helperDir 'hagency-update-helper.exe'
        Copy-Item -LiteralPath $binary -Destination $helperExe
        $installDir = Join-Path $data 'Installed Application'
        New-Item -ItemType Directory -Path $installDir | Out-Null
        $target = Join-Path $installDir 'hagency.exe'
        Copy-Item -LiteralPath $oldProbe -Destination $target
        $old = (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash
        $releaseParent = Join-Path $data 'release-parent'
        $env:HAGENCY_APP_DATA_DIR = $data
        $env:HAGENCY_UPDATE_TEST_MARKER = Join-Path $data 'restarted'
        $oldMarker = [System.IO.Path]::ChangeExtension($env:HAGENCY_UPDATE_TEST_MARKER, 'old')
        $parentScript = Join-Path $data 'parent.ps1'
        $quotedParent = $releaseParent.Replace("'", "''")
        "while (-not (Test-Path -LiteralPath '$quotedParent')) { Start-Sleep -Milliseconds 100 }" | Set-Content -LiteralPath $parentScript
        $parent = Start-Process powershell.exe -ArgumentList @('-NoProfile', '-File', "`"$parentScript`"") -PassThru -WindowStyle Hidden
        try {
            if ($scenario -eq 'tamper') {
                Set-Content -LiteralPath (Join-Path $updates '0.99.0/installer.exe') -Value 'corrupted installer'
            }
            $helper = Start-Process -FilePath $helperExe -ArgumentList @('--hagency-apply-update', "`"$updates`"", $parent.Id, "`"$target`"", "`"$helperDir`"") -PassThru -WindowStyle Hidden
            try {
                $deadline = [DateTime]::UtcNow.AddSeconds(30)
                while (-not (Test-Path -LiteralPath (Join-Path $helperDir 'ready')) -and -not $helper.HasExited -and [DateTime]::UtcNow -lt $deadline) {
                    Start-Sleep -Milliseconds 100
                    $helper.Refresh()
                }
                if ($scenario -ne 'tamper' -and -not (Test-Path -LiteralPath (Join-Path $helperDir 'ready'))) { throw 'Helper did not acknowledge readiness' }
                if ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -ne $old) { throw 'Installer ran before parent exit' }
                if ((Test-Path -LiteralPath $env:HAGENCY_UPDATE_TEST_MARKER) -or (Test-Path -LiteralPath $oldMarker)) { throw 'Restarted before parent exit' }
                if ($scenario -in @('install', 'busy')) { Set-Content -LiteralPath (Join-Path $helperDir 'committed') -Value 'committed' }
                Set-Content -LiteralPath $releaseParent -Value 'exit'
                if (-not $parent.WaitForExit(10000)) { throw 'Fixture parent did not exit' }
                if (-not $helper.WaitForExit(30000)) { throw 'Fixture helper did not finish' }
                if ($scenario -eq 'install') {
                    if ($helper.ExitCode -ne 0) { throw 'Installation failed' }
                    $deadline = [DateTime]::UtcNow.AddSeconds(10)
                    while (-not (Test-Path -LiteralPath $env:HAGENCY_UPDATE_TEST_MARKER) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
                    if (-not (Test-Path -LiteralPath $env:HAGENCY_UPDATE_TEST_MARKER)) { throw 'New executable did not restart' }
                    if ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -ne (Get-FileHash -LiteralPath $probe -Algorithm SHA256).Hash) { throw 'Wrong installed executable' }
                } else {
                    if ($helper.ExitCode -eq 0) { throw 'Invalid or canceled update was accepted' }
                    if ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -ne $old) { throw 'Canceled update changed the installation' }
                    if (Test-Path -LiteralPath $env:HAGENCY_UPDATE_TEST_MARKER) { throw 'Refused update started the new version' }
                    if ($scenario -eq 'busy') {
                        $deadline = [DateTime]::UtcNow.AddSeconds(10)
                        while (-not (Test-Path -LiteralPath $oldMarker) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
                        if (-not (Test-Path -LiteralPath $oldMarker)) { throw 'Refused installer did not restore the old version' }
                    } elseif (Test-Path -LiteralPath $oldMarker) {
                        throw 'Uncommitted update restarted the application'
                    }
                }
                $results += [pscustomobject]@{ scenario = $scenario; passed = $true; helperExit = $helper.ExitCode }
            } finally {
                if (-not $helper.HasExited) { Stop-Process -Id $helper.Id }
            }
        } finally {
            Set-Content -LiteralPath $releaseParent -Value 'exit'
            if (-not $parent.WaitForExit(10000)) { Stop-Process -Id $parent.Id }
        }
    }
    $results | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $testRoot 'results.json')
    Write-Output "Windows update handoff verified: $testRoot"
    $results | Format-Table
} finally {
    $env:HAGENCY_UPDATER_PUBLIC_KEY = $previousKey
    $env:HAGENCY_APP_DATA_DIR = $previousData
    $env:HAGENCY_UPDATE_TEST_MARKER = $previousMarker
    $env:RUSTUP_TOOLCHAIN = $previousToolchain
    $env:HAGENCY_UPDATE_TEST_BUSY = $previousBusy
    $env:TEMP = $previousTemp
    $env:TMP = $previousTmp
    Pop-Location
}
