# Install ShoutingToad for the current user (no administrator rights needed).
#
#   From a release zip:     powershell -ExecutionPolicy Bypass -File install_windows.ps1
#   From a source checkout: cargo build --release; powershell -ExecutionPolicy Bypass -File installer\install_windows.ps1
#
# If no shouting-toad.exe is found next to this script (or in target\release), the latest release is downloaded.
param(
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\ShoutingToad",
    [switch]$NoPath
)
$ErrorActionPreference = 'Stop'
$Repo = 'NDcompany/ShoutingToad'

$candidates = @("$PSScriptRoot\shouting-toad.exe", "$PSScriptRoot\..\target\release\shouting-toad.exe")
$source = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
$tmp = $null
if (-not $source) {
    Write-Host 'Downloading the latest ShoutingToad release...'
    $tmp = Join-Path $env:TEMP ("shouting-toad-" + [guid]::NewGuid())
    New-Item -ItemType Directory -Force $tmp | Out-Null
    $zip = Join-Path $tmp 'st.zip'
    Invoke-WebRequest "https://github.com/$Repo/releases/latest/download/shouting-toad-x86_64-pc-windows-msvc.zip" -OutFile $zip -UseBasicParsing
    Expand-Archive $zip -DestinationPath $tmp -Force
    $source = Join-Path $tmp 'shouting-toad.exe'
}

New-Item -ItemType Directory -Force $InstallDir | Out-Null
Copy-Item $source "$InstallDir\shouting-toad.exe" -Force
# Console build: terminals prefer .com, so CLI output and exit codes work in cmd/PowerShell.
# (a release zip has shouting-toad.com; a source build has shouting-toad-console.exe in the target release folder)
$console = @('shouting-toad.com', 'shouting-toad-console.exe') | ForEach-Object { Join-Path (Split-Path $source) $_ } | Where-Object { Test-Path $_ } | Select-Object -First 1
if ($console) { Copy-Item $console "$InstallDir\shouting-toad.com" -Force }
$uninstaller = @("$PSScriptRoot\uninstall_windows.ps1", "$tmp\uninstall_windows.ps1") | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if ($uninstaller) { Copy-Item $uninstaller "$InstallDir\uninstall_windows.ps1" -Force }

$shell = New-Object -ComObject WScript.Shell
$startMenu = "$env:APPDATA\Microsoft\Windows\Start Menu\Programs"
foreach ($lnkPath in @("$startMenu\ShoutingToad.lnk", "$env:USERPROFILE\Desktop\ShoutingToad.lnk")) {
    $lnk = $shell.CreateShortcut($lnkPath)
    $lnk.TargetPath = "$InstallDir\shouting-toad.exe"
    $lnk.WorkingDirectory = $InstallDir
    $lnk.Description = 'ShoutingToad SEO Spider'
    $lnk.Save()
}

if (-not $NoPath) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($userPath -split ';') -notcontains $InstallDir) {
        $newPath = if ($userPath) { "$($userPath.TrimEnd(';'));$InstallDir" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    }
}

$version = (& "$InstallDir\shouting-toad.exe" --version 2>$null) -replace 'shouting-toad ', ''
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\ShoutingToad'
New-Item -Path $key -Force | Out-Null
Set-ItemProperty $key DisplayName 'ShoutingToad SEO Spider'
Set-ItemProperty $key DisplayVersion "$version"
Set-ItemProperty $key Publisher 'N&D Co.'
Set-ItemProperty $key DisplayIcon "$InstallDir\shouting-toad.exe"
Set-ItemProperty $key InstallLocation $InstallDir
Set-ItemProperty $key URLInfoAbout "https://github.com/$Repo"
Set-ItemProperty $key UninstallString "powershell.exe -ExecutionPolicy Bypass -File `"$InstallDir\uninstall_windows.ps1`""
Set-ItemProperty $key NoModify 1
Set-ItemProperty $key NoRepair 1

if ($tmp) { Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue }
Write-Host "ShoutingToad installed to $InstallDir" -ForegroundColor Green
Write-Host 'Open it from the Start menu, or run "shouting-toad --help" in a new terminal.'
