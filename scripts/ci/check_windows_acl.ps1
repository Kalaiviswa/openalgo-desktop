# The data folder of an installed OpenAlgo Desktop is private to the
# Windows user who ran it (PLATFORM-04).
#
#   pwsh scripts/ci/check_windows_acl.ps1 [-Folder <path>]
#
# -Folder checks another folder instead; the workflow first runs it on a
# folder it opened to Everyone, which must fail, so the check can fail.
#
# The app keeps its databases (keys wrapped with the password, broker
# credentials encrypted, logs) in %APPDATA%\com.openalgo.desktop and sets no
# ACL of its own on Windows: it relies on the per-user profile's inherited
# ACL (src-tauri/src/security/fsperm.rs). This checks that assumption on the
# machine it runs on: no allow entry on the folder or on anything in it
# (openalgo.db and its -wal and -shm files included) for Everyone,
# Authenticated Users, Users, Guests, Anonymous, Interactive or Network.
# The user, SYSTEM and Administrators may have access.
#
# install-smoke.yml runs it after the installed app's first start on
# Windows Server 2022 and 2025. A redirected or roaming profile on a
# trader's machine is checked by hand (docs/release/platform-qualification.md).

param([string]$Folder = '')

$ErrorActionPreference = 'Stop'

$dataFolder = if ($Folder) { $Folder } else {
    Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'com.openalgo.desktop'
}
if (-not (Test-Path -LiteralPath $dataFolder)) {
    Write-Host "::error title=Data folder ACL::$dataFolder does not exist; the app was not started as this user."
    exit 1
}

$broad = @{
    'S-1-1-0'      = 'Everyone'
    'S-1-5-11'     = 'Authenticated Users'
    'S-1-5-32-545' = 'Users'
    'S-1-5-32-546' = 'Guests'
    'S-1-5-7'      = 'Anonymous'
    'S-1-5-4'      = 'Interactive'
    'S-1-5-2'      = 'Network'
}

$items = @(Get-Item -LiteralPath $dataFolder -Force) + @(Get-ChildItem -LiteralPath $dataFolder -Recurse -Force)
$problems = @()
foreach ($item in $items) {
    foreach ($ace in (Get-Acl -LiteralPath $item.FullName).Access) {
        if ($ace.AccessControlType -ne 'Allow') { continue }
        try {
            $sid = $ace.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value
        } catch {
            $sid = $ace.IdentityReference.Value
        }
        if ($broad.ContainsKey($sid)) {
            $problems += "$($item.FullName): $($broad[$sid]) is allowed $($ace.FileSystemRights)"
        }
    }
}

$acl = Get-Acl -LiteralPath $dataFolder
Write-Host "Data folder: $dataFolder (owner $($acl.Owner)); $($items.Count) items checked:"
Write-Host (($items | ForEach-Object { $_.Name }) -join ', ')
$acl.Access | Format-Table IdentityReference, AccessControlType, FileSystemRights, IsInherited -AutoSize | Out-String | Write-Host

if ($problems.Count -gt 0) {
    foreach ($p in $problems) { Write-Host "::error title=Data folder ACL::$p" }
    exit 1
}
Write-Host 'OK: only this user, SYSTEM and Administrators may open the data folder and its files.'
