# Install Homelab Music on Windows, from the latest GitHub release.
#
#   irm https://raw.githubusercontent.com/joe-lloyd/homelab-music/main/scripts/install-windows.ps1 | iex
#
# Downloads the installer from the latest release and runs it silently. No admin
# rights are needed: the NSIS bundle installs per-user.
#
# The build is unsigned -- there is no Windows code-signing certificate behind
# this. SmartScreen may warn the first time; the installer itself is what you
# just downloaded from your own repository. Uninstall through Settings > Apps
# like anything else.
#
# After this, the app updates itself from the tray.

$ErrorActionPreference = 'Stop'

$repo = 'joe-lloyd/homelab-music'

Write-Host '==> Looking up the latest release'
$release = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest" -Headers @{
    'User-Agent' = 'homelab-music-installer'
}

# The NSIS .exe is the installer; the .zip alongside it is the updater artifact
# and is not what we want here.
$asset = $release.assets | Where-Object { $_.name -like '*-setup.exe' } | Select-Object -First 1

if ($null -eq $asset) {
    Write-Error "No Windows installer in the latest release. See https://github.com/$repo/releases"
}

$out = Join-Path $env:TEMP $asset.name
Write-Host "==> Downloading $($asset.name)"
Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $out -UseBasicParsing

Write-Host '==> Installing'
# /S is NSIS silent mode. Waiting matters: without it the script reports success
# while the installer is still running, and a following launch would race it.
$process = Start-Process -FilePath $out -ArgumentList '/S' -Wait -PassThru

if ($process.ExitCode -ne 0) {
    Write-Error "Installer exited with code $($process.ExitCode)"
}

Remove-Item $out -Force -ErrorAction SilentlyContinue

Write-Host ''
Write-Host "Installed Homelab Music $($release.tag_name)."
Write-Host 'Find it in the Start menu. It lives in the tray once running.'
