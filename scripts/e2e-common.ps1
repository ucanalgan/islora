# Shared by the end-to-end scripts: window checks, clicks and screenshots.

Add-Type -AssemblyName System.Drawing
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
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte k, byte s, uint f, UIntPtr e);
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, System.Text.StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  public static long Root(int x, int y) { POINT p; p.X = x; p.Y = y; return GetAncestor(WindowFromPoint(p), 2).ToInt64(); }
  public static string Describe(int x, int y) {
    POINT p; p.X = x; p.Y = y;
    var root = GetAncestor(WindowFromPoint(p), 2);
    var name = new System.Text.StringBuilder(256);
    GetClassNameW(root, name, 256);
    uint pid; GetWindowThreadProcessId(root, out pid);
    string process = "?";
    try { process = System.Diagnostics.Process.GetProcessById((int)pid).ProcessName; } catch {}
    return name + " in " + process;
  }
  public static bool TestWindowAt(int x, int y) {
    POINT p; p.X = x; p.Y = y;
    var name = new System.Text.StringBuilder(256);
    GetClassNameW(GetAncestor(WindowFromPoint(p), 2), name, 256);
    return name.ToString().StartsWith("WindowsForms");
  }
  public static void Press(params byte[] keys) {
    foreach (var k in keys) keybd_event(k, 0, 0, UIntPtr.Zero);
    Array.Reverse(keys);
    foreach (var k in keys) keybd_event(k, 0, 2, UIntPtr.Zero);
  }
}
'@

$script:failed = 0
function Check([string]$Name, [bool]$Ok) {
    if ($Ok) { "  ok    $Name" } else { "  FAIL  $Name"; $script:failed++ }
}

function Wait-Until([scriptblock]$Condition, [int]$Milliseconds) {
    $deadline = (Get-Date).AddMilliseconds($Milliseconds)
    do {
        if (& $Condition) { return $true }
        Start-Sleep -Milliseconds 50
    } while ((Get-Date) -lt $deadline)
    $false
}

function Island { [Win]::FindWindowW('DynamicIslandWindow', [NullString]::Value) }
function Edge { [Win]::FindWindowW('DynamicIslandEdge', [NullString]::Value) }
function Visible { [Win]::IsWindowVisible((Island)) }

$centerX = [int]([Win]::GetSystemMetrics(0) / 2)

function Shot([string]$Name) {
    $bitmap = New-Object System.Drawing.Bitmap 460, 220
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.CopyFromScreen($centerX - 230, 0, 0, 0, $bitmap.Size)
    $bitmap.Save((Join-Path $Shots "$Name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $graphics.Dispose(); $bitmap.Dispose()
}

# Red channel of one screen pixel, for telling a drawn view from an empty one.
function Get-ScreenRed([int]$X, [int]$Y) {
    $bitmap = New-Object System.Drawing.Bitmap 1, 1
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.CopyFromScreen($X, $Y, 0, 0, $bitmap.Size)
    $red = $bitmap.GetPixel(0, 0).R
    $graphics.Dispose(); $bitmap.Dispose()
    $red
}

# Seconds left in the running Windows focus session, from its settings record.
function Get-FocusLeft {
    $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\CloudStore\Store\DefaultAccount\Current\default$windows.data.shell.focusactivesessions\windows.data.shell.focusactivesessions'
    $data = (Get-ItemProperty -LiteralPath $key -Name Data -ErrorAction SilentlyContinue).Data
    if (-not $data) { return -1 }
    for ($tag = $data.Length - 2; $tag -ge 0; $tag--) {
        if ($data[$tag] -ne 0xC6 -or $data[$tag + 1] -ne 0x1E) { continue }
        [uint64]$ticks = 0
        for ($index = 0; $index -lt 10; $index++) {
            $byte = $data[$tag + 2 + $index]
            $ticks = $ticks -bor ([uint64]($byte -band 0x7F) -shl ($index * 7))
            if (-not ($byte -band 0x80)) { break }
        }
        return [int](([DateTime]::FromFileTimeUtc([long]$ticks) - [DateTime]::UtcNow).TotalSeconds)
    }
    -1
}

function Stop-Island {
    $hwnd = Island
    if ($hwnd -ne [IntPtr]::Zero) { [void][Win]::PostMessageW($hwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) }
    Start-Sleep -Milliseconds 500
}

function Show-TestWindow([string]$Setup, [int]$Seconds) {
    # The first ShowWindow in a process obeys the hidden start, hence Show twice.
    $code = "Add-Type -AssemblyName System.Windows.Forms; `$f = New-Object Windows.Forms.Form; `$f.TopMost = `$true; $Setup; `$f.Show(); `$f.Hide(); `$f.Show(); `$f.Activate(); `$end = (Get-Date).AddSeconds($Seconds); while ((Get-Date) -lt `$end) { [Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 15 }"
    Start-Process powershell -ArgumentList '-NoProfile', '-Command', $code -WindowStyle Hidden -PassThru
}

# Only ever presses the button on the island itself, never on the user's windows.
function Click-Island([int]$X, [int]$Y) {
    if ([Win]::Root($X, $Y) -ne (Island).ToInt64()) { return $false }
    [void][Win]::SetCursorPos($X, $Y)
    Start-Sleep -Milliseconds 150
    [Win]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
    [Win]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
    $true
}
