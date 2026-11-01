# Checks the island against the real Spotify app. Unlike e2e.ps1 this DOES touch
# the user's music: it pauses it and skips a track, then puts things back. Run it
# only when they ask for it. The app is pinned to Spotify's session, so commands
# cannot reach any other player even if Windows changes the current session.
param(
    [string]$Exe = "$PSScriptRoot\..\target\release\islora.exe",
    [string]$Shots = "$env:TEMP\islora-spotify",
    [string]$Session = 'Spotify.exe'
)

New-Item -ItemType Directory -Force $Shots | Out-Null
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Runtime.WindowsRuntime
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Win {
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string c, string n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
  [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint f);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte k, byte s, uint f, UIntPtr e);
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
  public static long Root(int x, int y) { POINT p; p.X = x; p.Y = y; return GetAncestor(WindowFromPoint(p), 2).ToInt64(); }
  public static void Press(params byte[] keys) {
    foreach (var k in keys) keybd_event(k, 0, 0, UIntPtr.Zero);
    Array.Reverse(keys);
    foreach (var k in keys) keybd_event(k, 0, 2, UIntPtr.Zero);
  }
}
'@

$asTask = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and
    $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
} | Select-Object -First 1
function Await($Operation, [Type]$Type) {
    $task = $asTask.MakeGenericMethod($Type).Invoke($null, @($Operation))
    [void]$task.Wait(5000)
    $task.Result
}
$managerType = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]
$propertiesType = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionMediaProperties, Windows.Media.Control, ContentType = WindowsRuntime]
$manager = Await ($managerType::RequestAsync()) $managerType

# What Windows itself reports, independent of the island.
function Get-Player {
    foreach ($candidate in $manager.GetSessions()) {
        if ($candidate.SourceAppUserModelId -eq $Session) {
            $properties = Await ($candidate.TryGetMediaPropertiesAsync()) $propertiesType
            return [pscustomobject]@{
                Status = "$($candidate.GetPlaybackInfo().PlaybackStatus)"
                Track  = "$($properties.Artist) - $($properties.Title)"
            }
        }
    }
    $null
}

function Wait-Player([scriptblock]$Condition, [int]$Milliseconds = 3000) {
    $deadline = (Get-Date).AddMilliseconds($Milliseconds)
    do {
        $state = Get-Player
        if ($state -and (& $Condition $state)) { return $state }
        Start-Sleep -Milliseconds 150
    } while ((Get-Date) -lt $deadline)
    $null
}

$script:failed = 0
function Check([string]$Name, [bool]$Ok) {
    if ($Ok) { "  ok    $Name" } else { "  FAIL  $Name"; $script:failed++ }
}

$centerX = [int]([Win]::GetSystemMetrics(0) / 2)
function Shot([string]$Name) {
    $bitmap = New-Object System.Drawing.Bitmap 460, 220
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.CopyFromScreen($centerX - 230, 0, 0, 0, $bitmap.Size)
    $bitmap.Save((Join-Path $Shots "$Name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $graphics.Dispose(); $bitmap.Dispose()
}

$before = Get-Player
if (-not $before) { "No $Session session. Start Spotify and play something first."; exit 2 }
"Before: $($before.Status) | $($before.Track)"

$saved = New-Object Win+POINT
[void][Win]::GetCursorPos([ref]$saved)
Get-Process islora -ErrorAction SilentlyContinue | Stop-Process -Force
# A killed copy holds the single-instance lock for a moment.
Start-Sleep -Milliseconds 800

function Click-Button([int]$Offset, [long]$Window) {
    if ([Win]::Root($centerX + $Offset, 146) -ne $Window) { return $false }
    [void][Win]::SetCursorPos($centerX + $Offset, 146)
    Start-Sleep -Milliseconds 150
    [Win]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
    [Win]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
    $true
}

try {
    Start-Process -FilePath $Exe -ArgumentList '--only-session', $Session, '--capturable'
    Start-Sleep -Milliseconds 1800
    $island = [Win]::FindWindowW('IsloraWindow', [NullString]::Value)
    Check 'Spotify shows in the island by itself' ([Win]::IsWindowVisible($island))
    Shot 's1-compact'

    [Win]::Press(0x5B, 0x12, 0x20)
    Start-Sleep -Milliseconds 800
    Shot 's2-expanded'
    $window = $island.ToInt64()
    Check 'the shortcut opens the player' ([Win]::Root($centerX, 146) -eq $window)

    $original = $before.Status
    $other = if ($original -eq 'Playing') { 'Paused' } else { 'Playing' }

    [void](Click-Button 0 $window)
    $state = Wait-Player { param($s) $s.Status -eq $other }
    Check "the middle button changes Spotify to $other" ($null -ne $state)
    Start-Sleep -Milliseconds 500
    Shot 's3-toggled'

    [void](Click-Button 0 $window)
    $state = Wait-Player { param($s) $s.Status -eq $original }
    Check "the middle button returns Spotify to $original" ($null -ne $state)

    $track = (Get-Player).Track
    [void](Click-Button 56 $window)
    $state = Wait-Player { param($s) $s.Track -ne $track }
    Check 'the right button skips to another track' ($null -ne $state)
    if ($state) { "        now: $($state.Track)" }
    Start-Sleep -Milliseconds 1000
    Shot 's4-next'

    [void](Click-Button -56 $window)
    $state = Wait-Player { param($s) $s.Track -eq $track }
    Check 'the left button goes back to the first track' ($null -ne $state)
    Start-Sleep -Milliseconds 700
    Shot 's5-previous'

    $island = [Win]::FindWindowW('IsloraWindow', [NullString]::Value)
    Check 'the app is still running' ($island -ne [IntPtr]::Zero)
}
finally {
    [void][Win]::SetCursorPos($saved.X, $saved.Y)
    $island = [Win]::FindWindowW('IsloraWindow', [NullString]::Value)
    if ($island -ne [IntPtr]::Zero) { [void][Win]::PostMessageW($island, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) }
    Start-Sleep -Milliseconds 500
    Get-Process islora -ErrorAction SilentlyContinue | Stop-Process -Force
}

$after = Get-Player
"After:  $($after.Status) | $($after.Track)"
Check 'music is back to how it was' ($after.Status -eq $before.Status -and $after.Track -eq $before.Track)
"Screenshots: $Shots"
if ($script:failed) { "$script:failed check(s) failed"; exit 1 }
'All checks passed'
