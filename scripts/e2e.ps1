# End-to-end behaviour checks against the real window. Moves the mouse, sends
# the global shortcut and briefly covers the screen, so leave the machine alone
# for about a minute. It also toggles Caps Lock and nudges the volume one step,
# restoring both. Exits 1 if any check fails. The app always runs with
# --only-session, so real media players are ignored and never controlled.
param(
    [string]$Exe = "$PSScriptRoot\..\target\release\islora.exe",
    [string]$Shots = "$env:TEMP\islora-e2e"
)

. "$PSScriptRoot\media-fixture.ps1"

New-Item -ItemType Directory -Force $Shots | Out-Null
. "$PSScriptRoot\e2e-common.ps1"

$saved = New-Object Win+POINT
[void][Win]::GetCursorPos([ref]$saved)
$foreground = [Win]::GetForegroundWindow()

try {
    'Startup'
    Get-Process islora -ErrorAction SilentlyContinue | Stop-Process -Force
    # A killed copy holds the single-instance lock for a moment.
    Start-Sleep -Milliseconds 800
    Start-Process -FilePath $Exe -ArgumentList $NoSession
    Start-Sleep -Seconds 1
    Check 'starts hidden' (-not (Visible))
    Check 'edge strip is on screen' ([Win]::IsWindowVisible((Edge)))

    'Second instance'
    $second = Start-Process -FilePath $Exe -ArgumentList $NoSession -PassThru
    Start-Sleep -Milliseconds 800
    Check 'second copy exits at once' $second.HasExited

    'Keyboard shortcut'
    # The pill rightly stays open under the pointer, wherever the user left it.
    [void][Win]::SetCursorPos($centerX, 600)
    Start-Sleep -Milliseconds 150
    [Win]::Press(0x5B, 0x12, 0x20)
    Start-Sleep -Milliseconds 700
    Check 'Win+Alt+Space shows the clock' (Visible)
    Shot '1-clock'
    Check 'focus stays where it was' ([Win]::GetForegroundWindow() -eq $foreground)
    # Real indicators can now interrupt, so allow for one before it hides.
    Check 'clock hides by itself' (Wait-Until { -not (Visible) } 7000)

    'Top edge'
    [void][Win]::SetCursorPos($centerX, 600)
    Start-Sleep -Milliseconds 100
    [void][Win]::SetCursorPos($centerX, 0)
    Start-Sleep -Milliseconds 40
    [void][Win]::SetCursorPos($centerX + 1, 0)
    Start-Sleep -Milliseconds 600
    Check 'resting on the top edge reveals the clock' (Visible)
    [void][Win]::SetCursorPos($centerX, 600)
    Start-Sleep -Milliseconds 3400
    Check 'hides after the pointer goes' (-not (Visible))

    'Passing over the edge'
    [void][Win]::SetCursorPos($centerX, 0)
    Start-Sleep -Milliseconds 20
    [void][Win]::SetCursorPos($centerX + 1, 0)
    Start-Sleep -Milliseconds 20
    [void][Win]::SetCursorPos($centerX, 600)
    Start-Sleep -Milliseconds 500
    Check 'a quick pass does not open it' (-not (Visible))
    Stop-Island

    'Transitions'
    Start-Process -FilePath $Exe -ArgumentList ($NoSession + '--demo')
    Start-Sleep -Milliseconds 1500
    $hwnd = (Island).ToInt64()
    Shot '2-compact'
    Check 'compact is visible' (Visible)
    $takesClicks = [Win]::Root($centerX, 25) -eq $hwnd
    Check 'pill takes clicks' $takesClicks
    if (-not $takesClicks) { "        in the way: $([Win]::Describe($centerX, 25))" }
    Check 'outside the pill reaches the app below' ([Win]::Root($centerX, 120) -ne $hwnd)

    Start-Sleep -Milliseconds 3500
    Shot '3-expanded'
    Check 'expanded takes clicks in its full area' ([Win]::Root($centerX, 120) -eq $hwnd)

    Start-Sleep -Milliseconds 3200
    Shot '4-alert'
    Start-Sleep -Milliseconds 3500
    Shot '5-notification'
    # The demo loops, so watch for its gap rather than sleeping towards it.
    Check 'hides when the last activity ends' (Wait-Until { -not (Visible) } 5000)
    Stop-Island

    'Staying out of the way'
    Start-Process -FilePath $Exe -ArgumentList ($NoSession + '--demo')
    Start-Sleep -Milliseconds 1500
    Check 'visible before a full screen app' (Visible)
    $screen = Show-TestWindow "`$f.FormBorderStyle = 'None'; `$f.BackColor = 'Black'; `$f.StartPosition = 'Manual'; `$f.Bounds = [Windows.Forms.Screen]::PrimaryScreen.Bounds" 2
    Start-Sleep -Milliseconds 1600
    Check 'test window really covers the screen' ([Win]::TestWindowAt($centerX, 500))
    Check 'hides for a full screen app' (-not (Visible))
    Check 'edge strip hides too' (-not [Win]::IsWindowVisible((Edge)))
    $screen.WaitForExit()
    Start-Sleep -Milliseconds 800
    Check 'edge strip returns after full screen' ([Win]::IsWindowVisible((Edge)))
    Stop-Island

    'Window drag'
    Start-Process -FilePath $Exe -ArgumentList ($NoSession + '--demo')
    Start-Sleep -Milliseconds 1000
    $dragged = Show-TestWindow "`$f.StartPosition = 'Manual'; `$f.Location = '200,300'; `$f.Size = '320,200'" 4
    Start-Sleep -Milliseconds 1200
    Check 'visible before the drag' (Visible)
    # Only ever press the button on our own test window, never on the user's.
    $titleBarIsOurs = [Win]::TestWindowAt(320, 315)
    Check 'test window is under the pointer' $titleBarIsOurs
    if ($titleBarIsOurs) {
        [void][Win]::SetCursorPos(320, 315)
        Start-Sleep -Milliseconds 100
        [Win]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 120
        [void][Win]::SetCursorPos(380, 340)
        Start-Sleep -Milliseconds 600
        Check 'hides while a window is dragged' (-not (Visible))
        [Win]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 600
        Check 'returns after the drag' (Visible)
    }
    $dragged.WaitForExit()
    Stop-Island

    'Media'
    $fixture = New-MediaFixture $Shots
    $playerLog = Join-Path $Shots 'player.txt'
    $player = Start-TestPlayer $fixture 40 $playerLog
    Start-Sleep -Milliseconds 1200
    Start-Process -FilePath $Exe -ArgumentList '--only-session', $TestSession
    Start-Sleep -Milliseconds 1500
    Check 'a playing session shows by itself' (Visible)
    Shot '6-media-compact'
    Check 'the compact pill leaves the rest clickable' ([Win]::Root($centerX, 120) -ne (Island).ToInt64())

    [Win]::Press(0x5B, 0x12, 0x20)
    Start-Sleep -Milliseconds 800
    Shot '7-media-expanded'
    $hwnd = (Island).ToInt64()
    Check 'the shortcut opens the player' ([Win]::Root($centerX, 146) -eq $hwnd)

    # Only ever clicks the pill, whose session is the test player alone.
    function Click-MediaButton([int]$Offset) {
        if ([Win]::Root($centerX + $Offset, 146) -ne $hwnd) { return }
        [void][Win]::SetCursorPos($centerX + $Offset, 146)
        Start-Sleep -Milliseconds 150
        [Win]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
        [Win]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 700
    }
    function Player-Said([string]$Line) { @(Get-Content $playerLog | Where-Object { $_ -eq $Line }).Count }

    $paused = Player-Said 'state paused'
    Click-MediaButton 0
    Check 'the middle button pauses' ((Player-Said 'state paused') -gt $paused)
    Shot '8-media-paused'
    $playing = Player-Said 'state playing'
    Click-MediaButton 0
    Check 'the middle button resumes' ((Player-Said 'state playing') -gt $playing)
    Click-MediaButton 56
    Check 'the right button skips forward' ((Player-Said 'command next') -gt 0)
    Click-MediaButton -56
    Check 'the left button skips back' ((Player-Said 'command previous') -gt 0)

    # The bar spans 352 px from 176 px left of the centre, on a 900 s test track.
    function Bar-X([double]$Fraction) { [int]($centerX - 176 + 352 * $Fraction) }
    function Last-Seek {
        $line = @(Get-Content $playerLog | Where-Object { $_ -like 'command position *' }) | Select-Object -Last 1
        if ($line) { [int]($line -replace 'command position ', '') } else { -1 }
    }
    function Seek-Count { @(Get-Content $playerLog | Where-Object { $_ -like 'command position *' }).Count }
    function Near([int]$Value, [int]$Expected) { [math]::Abs($Value - $Expected) -le 15 }

    if ([Win]::Root((Bar-X 0.75), 114) -eq $hwnd) {
        [void][Win]::SetCursorPos((Bar-X 0.75), 114)
        Start-Sleep -Milliseconds 150
        [Win]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
        [Win]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 700
    }
    Check 'clicking the bar jumps there' (Near (Last-Seek) 675)
    Check 'the pill stays open after a jump' ([Win]::Root($centerX, 146) -eq $hwnd)

    $seeks = Seek-Count
    [void][Win]::SetCursorPos((Bar-X 0.25), 114)
    Start-Sleep -Milliseconds 150
    [Win]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 120
    foreach ($fraction in 0.3, 0.35, 0.4, 0.45, 0.5) {
        [void][Win]::SetCursorPos((Bar-X $fraction), 114)
        Start-Sleep -Milliseconds 60
    }
    Start-Sleep -Milliseconds 250
    Shot '11-media-drag'
    Check 'dragging sends nothing until released' ((Seek-Count) -eq $seeks)
    [Win]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 700
    Check 'releasing jumps once' ((Seek-Count) -eq ($seeks + 1))
    Check 'a drag lands where it was released' (Near (Last-Seek) 450)

    [void][Win]::SetCursorPos($centerX, 600)
    Start-Sleep -Milliseconds 1000
    Check 'collapses after the pointer leaves' ([Win]::Root($centerX, 120) -ne $hwnd)
    Check 'stays visible while playing' (Visible)

    Stop-Process -Id $player.Id -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 1500
    Check 'hides when the session ends' (-not (Visible))
    Stop-Island

    'Indicators'
    Start-Process -FilePath $Exe -ArgumentList $NoSession
    Start-Sleep -Milliseconds 1200
    Check 'hidden before any change' (-not (Visible))

    $caps = [Console]::CapsLock
    [Win]::Press(0x14)
    Check 'Caps Lock shows by itself' (Wait-Until { Visible } 1000)
    Start-Sleep -Milliseconds 450
    Shot '9-caps-lock'
    [Win]::Press(0x14)
    Start-Sleep -Milliseconds 300
    Check 'Caps Lock is back as it was' ([Console]::CapsLock -eq $caps)
    Check 'the alert leaves by itself' (Wait-Until { -not (Visible) } 4000)

    # Volume down does nothing at 0% and up does nothing at 100%, and no change
    # means no event. Going up first fails only at the very top. Either way the
    # opposite key then undoes it, so the volume ends where it started.
    [Win]::Press(0xAF)
    $undo = 0xAE
    if (-not (Wait-Until { Visible } 700)) {
        [Win]::Press(0xAE)
        $undo = 0xAF
    }
    Check 'a volume change shows by itself' (Wait-Until { Visible } 700)
    Start-Sleep -Milliseconds 450
    Shot '10-volume'
    [Win]::Press($undo)
    Start-Sleep -Milliseconds 200
    Check 'the volume bar leaves by itself' (Wait-Until { -not (Visible) } 4000)
    Stop-Island

    'Shutdown'
    Check 'closes cleanly' (-not (Get-Process islora -ErrorAction SilentlyContinue))
}
finally {
    [Win]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
    [void][Win]::SetCursorPos($saved.X, $saved.Y)
    Get-Process islora, test_player -ErrorAction SilentlyContinue | Stop-Process -Force
}

"Screenshots: $Shots"
if ($script:failed) { "$script:failed check(s) failed"; exit 1 }
'All checks passed'
