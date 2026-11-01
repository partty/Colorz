<#
.SYNOPSIS
Smoke-tests the Colorz installer on a Windows CI runner.

.DESCRIPTION
Installs Colorz for the current user, seeds palettes and settings, saves
screenshots of every page and dialog, then checks launching, single
instance, reinstalling over a running copy, background start (and a later
launch showing its window) and uninstalling. Exits non-zero with a message
naming the failed check.

Each Colorz process runs with PATH limited to the Windows directories, so
it must find every DLL next to the installed exe, and with its stdout and
stderr captured under <OutDir>\logs. A GTK/GLib warning, a missing icon or
a failed load on stderr fails the run.

.EXAMPLE
windows\smoke.ps1 -Installer out\colorz-0.1.0-setup-x64.exe -OutDir smoke
#>
param(
    [Parameter(Mandatory)] [string] $Installer,
    [Parameter(Mandatory)] [string] $OutDir
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Fixtures = Join-Path $PSScriptRoot 'smoke'
$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\Colorz'
$Exe = Join-Path $InstallDir 'bin\colorz.exe'
$Uninstaller = Join-Path $InstallDir 'unins000.exe'
$Shortcut = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Colorz.lnk'
$Notices = Join-Path $InstallDir 'share\licenses\colorz\THIRD-PARTY-RUST.txt'
$DataDir = Join-Path $env:APPDATA 'colorz'
$PalettesDir = Join-Path $DataDir 'palettes'
$RunKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$RunCommand = '"{0}" --background' -f $Exe
$BadStderr = 'Gtk-WARNING|Gtk-CRITICAL|GLib-GIO-WARNING|not present in theme|Failed to load'
$Shots = Join-Path $OutDir 'shots'
$LogDir = Join-Path $OutDir 'logs'
$Pngs = 'pick.png', 'wheel.png', 'palettes.png', 'settings.png', 'shortcuts.png', 'about.png'

# The check that is running, named in the failure message.
$script:Check = 'setup'

function Fail([string] $Message) {
    throw $Message
}

# Polls $Condition every 250 ms until it is true or $TimeoutSec runs out.
function Wait-Until([scriptblock] $Condition, [int] $TimeoutSec) {
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while (-not (& $Condition)) {
        if ((Get-Date) -gt $deadline) { return $false }
        Start-Sleep -Milliseconds 250
    }
    return $true
}

# Reads a log that a running process may still have open for writing.
function Read-Log([string] $Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return @() }
    $stream = [System.IO.File]::Open($Path, 'Open', 'Read', 'ReadWrite')
    try {
        $reader = [System.IO.StreamReader]::new($stream)
        return @($reader.ReadToEnd() -split "`r?`n" | Where-Object { $_ -ne '' })
    } finally {
        $stream.Dispose()
    }
}

function Get-RunValue {
    $item = Get-ItemProperty -Path $RunKey -Name Colorz -ErrorAction SilentlyContinue
    if ($item) { return $item.Colorz }
    return $null
}

# Callers wrap these in @(): PowerShell unrolls a returned array.
function Get-Colorz {
    Get-Process colorz -ErrorAction SilentlyContinue
}

# Every process whose executable lies in the install dir. Only colorz.exe
# should ever run from there.
function Get-InstalledProcess {
    Get-Process | Where-Object {
            $path = $null
            try { $path = $_.Path } catch { }
            $path -and $path.StartsWith("$InstallDir\", [StringComparison]::OrdinalIgnoreCase)
        }
}

function Show-InstalledProcess {
    $procs = @(Get-InstalledProcess)
    if ($procs.Count -eq 0) {
        Write-Host '  No process runs from the install dir'
    }
    foreach ($p in $procs) {
        Write-Host ('  Running from the install dir: {0} (pid {1})' -f $p.Path, $p.Id)
    }
}

function Stop-Colorz {
    Get-Colorz | Stop-Process -Force -ErrorAction SilentlyContinue
    Get-InstalledProcess | Stop-Process -Force -ErrorAction SilentlyContinue
}

# Starts the installed exe with stdout and stderr captured to
# <LogDir>\<Name>.stdout.log and <Name>.stderr.log.
function Start-Colorz([string] $Name, [string[]] $Arguments = @()) {
    $params = @{
        FilePath               = $Exe
        PassThru               = $true
        RedirectStandardOutput = Join-Path $LogDir "$Name.stdout.log"
        RedirectStandardError  = Join-Path $LogDir "$Name.stderr.log"
    }
    if ($Arguments.Count -gt 0) { $params.ArgumentList = $Arguments }
    $p = Start-Process @params
    # Caches the handle, or ExitCode stays empty once the process is gone.
    $null = $p.Handle
    Write-Host ('  Started colorz.exe {0} (pid {1})' -f ($Arguments -join ' '), $p.Id)
    return $p
}

# Runs an installer or uninstaller and returns its exit code.
function Invoke-Setup([string] $Path, [string[]] $Arguments, [int] $TimeoutSec) {
    Write-Host ('  Running {0} {1}' -f $Path, ($Arguments -join ' '))
    $p = Start-Process -FilePath $Path -ArgumentList $Arguments -PassThru
    $null = $p.Handle
    if (-not $p.WaitForExit($TimeoutSec * 1000)) {
        Fail ('{0} did not finish within {1} s' -f (Split-Path -Leaf $Path), $TimeoutSec)
    }
    return $p.ExitCode
}

function Assert-Running([System.Diagnostics.Process] $Process) {
    $Process.Refresh()
    if ($Process.HasExited) {
        Fail ('colorz.exe (pid {0}) exited with code {1}' -f $Process.Id, $Process.ExitCode)
    }
}

# Fails on any captured stderr line that matches $BadStderr.
function Assert-CleanStderr {
    $hits = @()
    foreach ($f in Get-ChildItem -LiteralPath $LogDir -Filter '*.stderr.log') {
        foreach ($line in Read-Log $f.FullName) {
            if ($line -match $BadStderr) { $hits += '  {0}: {1}' -f $f.Name, $line }
        }
    }
    if ($hits.Count -gt 0) {
        Fail ("stderr has warnings:`n" + ($hits -join "`n"))
    }
}

function Show-Logs {
    foreach ($f in Get-ChildItem -LiteralPath $LogDir -Filter '*.log' -ErrorAction SilentlyContinue) {
        $lines = @(Read-Log $f.FullName)
        Write-Host ('::group::{0} ({1} lines)' -f $f.Name, $lines.Count)
        $lines | ForEach-Object { Write-Host $_ }
        Write-Host '::endgroup::'
    }
    foreach ($f in Get-ChildItem -LiteralPath $OutDir -Filter '*install.log' -ErrorAction SilentlyContinue) {
        Write-Host ('::group::{0} (last 40 lines)' -f $f.Name)
        Read-Log $f.FullName | Select-Object -Last 40 | ForEach-Object { Write-Host $_ }
        Write-Host '::endgroup::'
    }
}

function Start-Check([string] $Name) {
    $script:Check = $Name
    Write-Host "== Check $Name"
}

$failed = $false
try {
    $Installer = (Resolve-Path -LiteralPath $Installer).Path
    New-Item -ItemType Directory -Force -Path $OutDir, $LogDir | Out-Null
    $OutDir = (Resolve-Path -LiteralPath $OutDir).Path
    $LogDir = Join-Path $OutDir 'logs'
    $Shots = Join-Path $OutDir 'shots'

    # Inherited by every process started from here on: Colorz must find all
    # of its DLLs next to the exe, not in MSYS2 or elsewhere on PATH.
    $env:PATH = 'C:\Windows\System32;C:\Windows'

    Start-Check '1 (install)'
    $code = Invoke-Setup $Installer @('/VERYSILENT', '/CURRENTUSER', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$OutDir\install.log`"") 300
    if ($code -ne 0) { Fail "the installer exited with code $code" }
    if (-not (Test-Path -LiteralPath $Exe)) { Fail "$Exe is missing" }
    if (-not (Test-Path -LiteralPath $Shortcut)) { Fail "$Shortcut is missing" }
    if (-not (Test-Path -LiteralPath $Notices)) { Fail "$Notices is missing" }
    $bus = Join-Path $InstallDir 'bin\gdbus.exe'
    if (Test-Path -LiteralPath $bus) { Fail "$bus is installed; Colorz must not start a session bus" }
    $run = Get-RunValue
    if ($null -ne $run) { Fail "the Run value already exists after installing: $run" }

    Start-Check '2 (seed)'
    if (Test-Path -LiteralPath $DataDir) { Remove-Item -LiteralPath $DataDir -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $PalettesDir | Out-Null
    Copy-Item -Path (Join-Path $Fixtures 'palettes\*.json') -Destination $PalettesDir
    Copy-Item -LiteralPath (Join-Path $Fixtures 'config.toml') -Destination (Join-Path $DataDir 'config.toml')
    Get-ChildItem -LiteralPath $DataDir -Recurse -File | ForEach-Object { Write-Host "  Seeded $($_.FullName)" }

    Start-Check '3 (screenshots)'
    $env:COLORZ_DEBUG_SNAPSHOT = $Shots
    $env:COLORZ_DEBUG_SNAPSHOT_DIALOGS = '1'
    try {
        $snap = Start-Colorz 'snapshot'
    } finally {
        Remove-Item Env:COLORZ_DEBUG_SNAPSHOT, Env:COLORZ_DEBUG_SNAPSHOT_DIALOGS
    }
    if (-not $snap.WaitForExit(60000)) { Fail 'the snapshot run did not exit within 60 s' }
    Write-Host "  The snapshot run exited with code $($snap.ExitCode)"
    $missing = @()
    foreach ($png in $Pngs) {
        $path = Join-Path $Shots $png
        if (Test-Path -LiteralPath $path) {
            $size = (Get-Item -LiteralPath $path).Length
            Write-Host ('  {0}: {1} bytes' -f $png, $size)
            if ($size -eq 0) { $missing += $png }
        } else {
            Write-Host "  ${png}: missing"
            $missing += $png
        }
    }
    if ($snap.ExitCode -ne 0) { Fail "the snapshot run exited with code $($snap.ExitCode)" }
    if ($missing.Count -gt 0) { Fail ('missing or empty screenshots: {0}' -f ($missing -join ', ')) }
    foreach ($f in Get-ChildItem -LiteralPath $LogDir -Filter '*.stderr.log') {
        Write-Host ('  {0}: {1} lines' -f $f.Name, @(Read-Log $f.FullName).Count)
    }
    Assert-CleanStderr

    Start-Check '4 (launch)'
    $first = Start-Colorz 'launch'
    Start-Sleep -Seconds 5
    Assert-Running $first
    Write-Host ('  MainWindowHandle: {0}' -f $first.MainWindowHandle)
    $run = Get-RunValue
    Write-Host "  Run value: $run"
    if ($null -eq $run) { Fail 'the Run value is missing' }
    if ($run -ne $RunCommand) { Fail "the Run value is '$run', expected '$RunCommand'" }
    Assert-CleanStderr

    Start-Check '5 (second launch)'
    $second = Start-Colorz 'second'
    Start-Sleep -Seconds 5
    $procs = @(Get-Colorz)
    Write-Host ('  colorz processes: {0} (pids {1})' -f $procs.Count, (($procs | ForEach-Object Id) -join ', '))
    if ($procs.Count -ne 1) { Fail "$($procs.Count) colorz processes are running, expected 1" }
    Assert-Running $first
    $others = @(Get-InstalledProcess | Where-Object { $_.ProcessName -ne 'colorz' })
    if ($others.Count -gt 0) {
        Fail ('other processes run from the install dir: {0}' -f (($others | ForEach-Object Path) -join ', '))
    }
    $second.Refresh()
    if (-not $second.HasExited) { Fail 'the second launch is still running' }
    Write-Host "  The second launch exited with code $($second.ExitCode)"
    if ($second.ExitCode -ne 0) { Fail "the second launch exited with code $($second.ExitCode)" }
    Assert-CleanStderr

    Start-Check '6 (reinstall)'
    Show-InstalledProcess
    $code = Invoke-Setup $Installer @('/VERYSILENT', '/CURRENTUSER', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$OutDir\reinstall.log`"") 300
    if ($code -ne 0) {
        Show-InstalledProcess
        Fail "the installer exited with code $code"
    }
    if (-not (Test-Path -LiteralPath $Exe)) { Fail "$Exe is missing after reinstalling" }
    if (-not (Wait-Until { @(Get-Colorz).Count -eq 0 } 5)) {
        Fail ('colorz is still running after reinstalling (pids {0})' -f ((@(Get-Colorz) | ForEach-Object Id) -join ', '))
    }
    Assert-CleanStderr

    Start-Check '7 (background)'
    $bg = Start-Colorz 'background' @('--background')
    Start-Sleep -Seconds 5
    Assert-Running $bg
    $handle = (Get-Process -Id $bg.Id).MainWindowHandle
    Write-Host "  MainWindowHandle: $handle"
    if ($handle -ne [IntPtr]::Zero) { Fail "the --background start shows a window (MainWindowHandle $handle)" }
    # A plain launch now asks the running Colorz to show its window, and exits.
    $show = Start-Colorz 'show'
    if (-not (Wait-Until { (Get-Process -Id $bg.Id).MainWindowHandle -ne [IntPtr]::Zero } 10)) {
        Fail 'a second launch did not bring up the window of the --background start'
    }
    Write-Host ('  MainWindowHandle after a second launch: {0}' -f (Get-Process -Id $bg.Id).MainWindowHandle)
    if (-not $show.WaitForExit(5000)) { Fail 'the second launch is still running' }
    if ($show.ExitCode -ne 0) { Fail "the second launch exited with code $($show.ExitCode)" }
    $procs = @(Get-Colorz)
    if ($procs.Count -ne 1) { Fail "$($procs.Count) colorz processes are running, expected 1" }
    Assert-CleanStderr

    Start-Check '8 (uninstall)'
    Assert-Running $bg
    Show-InstalledProcess
    $code = Invoke-Setup $Uninstaller @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$OutDir\uninstall.log`"") 60
    if ($code -ne 0) { Fail "the uninstaller exited with code $code" }
    # unins000.exe hands over to a copy of itself in %TEMP% and exits, so
    # wait for the result instead.
    if (-not (Wait-Until { -not (Test-Path -LiteralPath $InstallDir) } 30)) {
        Show-InstalledProcess
        Get-ChildItem -LiteralPath $InstallDir -Recurse -File -ErrorAction SilentlyContinue |
            Select-Object -First 20 | ForEach-Object { Write-Host "  Left behind: $($_.FullName)" }
        Fail "$InstallDir still exists 30 s after uninstalling"
    }
    if (Test-Path -LiteralPath $Shortcut) { Fail "$Shortcut still exists after uninstalling" }
    $run = Get-RunValue
    if ($null -ne $run) { Fail "the Run value still exists after uninstalling: $run" }
    if (-not (Test-Path -LiteralPath $PalettesDir)) { Fail "$PalettesDir was removed by uninstalling" }
    Write-Host ('  {0} kept with {1} files' -f $PalettesDir, @(Get-ChildItem -LiteralPath $PalettesDir -File).Count)
    if (@(Get-Colorz).Count -ne 0) { Fail 'colorz is still running after uninstalling' }
    Assert-CleanStderr

    Write-Host 'All smoke checks passed'
} catch {
    $failed = $true
    $message = "Smoke check $script:Check failed: $($_.Exception.Message)"
    Write-Host "::error::$($message -replace "`n", '%0A')"
} finally {
    try {
        Stop-Colorz
        Show-Logs
    } catch {
        Write-Host "Cleanup failed: $($_.Exception.Message)"
    }
}

if ($failed) { exit 1 }
exit 0
