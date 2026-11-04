# End-to-end checks of the island: two activities at once, the bubble,
# the home view, the timer and calendar reminders. Uses the silent test player
# and a made-up calendar file, so real music and real events are never touched.
# Moves the mouse: leave the machine alone for about 40 seconds.
param(
    [string]$Exe = "$PSScriptRoot\..\target\release\islora.exe",
    [string]$Shots = "$env:TEMP\islora-e2e"
)

. "$PSScriptRoot\media-fixture.ps1"

New-Item -ItemType Directory -Force $Shots | Out-Null
. "$PSScriptRoot\e2e-common.ps1"

# Positions relative to the screen centre, from the layout in island-core.
$PillY = 26
$ShortcutX = $centerX + 162
$ShortcutY = 44
$PresetY = 138
$Preset5X = $centerX - 132
$Preset25X = $centerX + 44
$ButtonY = 146
$TimerButtonY = 93
$CancelX = $centerX - 122
$PauseX = $centerX + 122
$ShuffleX = $centerX - 108
$RepeatX = $centerX + 108
$TabY = 104
$FocusTabX = $centerX - 141
$TimerTabX = $centerX - 61
# Inside a chosen tab's chip, left of its text.
$TimerChipX = $centerX - 102

$saved = New-Object Win+POINT
[void][Win]::GetCursorPos([ref]$saved)
$fixture = New-MediaFixture $Shots
$calendar = New-CalendarFixture $Shots
$playerLog = Join-Path $Shots 'player.txt'

try {
    Get-Process islora, test_player -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 800
    [void][Win]::SetCursorPos($centerX, 600)

    'Two activities'
    $player = Start-TestPlayer $fixture 120 $playerLog
    Start-Sleep -Milliseconds 1200
    Start-Process -FilePath $Exe -ArgumentList '--only-session', $TestSession, '--capturable', '--calendar-file', "`"$calendar`"", '--local-timers'
    Start-Sleep -Milliseconds 2200
    $hwnd = (Island).ToInt64()
    # The pill fits its content, so the bubble is found by looking along the top.
    $BubbleX = $centerX
    for ($x = $centerX + 225; $x -gt $centerX -and $BubbleX -eq $centerX; $x -= 2) {
        if ([Win]::Root($x, $PillY) -eq $hwnd) { $BubbleX = $x - 18 }
    }
    Check 'the bubble is there' ($BubbleX -gt $centerX + 60)
    Check 'the pill made room for it' ([Win]::Root($centerX - 120, $PillY) -eq $hwnd)
    Check 'beside them reaches the app below' ([Win]::Root($BubbleX + 30, $PillY) -ne $hwnd)
    Shot '12-two-activities'

    'Bubble'
    Check 'the bubble can be clicked' (Click-Island $BubbleX $PillY)
    Start-Sleep -Milliseconds 800
    Check 'it opens what it stands for' ([Win]::Root($centerX, 120) -eq $hwnd)
    Shot '13-bubble-opened'
    [void][Win]::SetCursorPos($centerX, 600)
    Start-Sleep -Milliseconds 1100
    Check 'it closes when the pointer leaves' ([Win]::Root($centerX, 120) -ne $hwnd)
    Check 'the bubble returns' ([Win]::Root($BubbleX, $PillY) -eq $hwnd)

    'Home view over music'
    [Win]::Press(0x5B, 0x12, 0x20)
    Start-Sleep -Milliseconds 800
    Shot '14-media-with-shortcut'
    Check 'shuffle can be clicked' (Click-Island $ShuffleX $ButtonY)
    Check 'the player is told' (Wait-Until { (Get-Content $playerLog) -contains 'command shuffle true' } 2000)
    Check 'repeat can be clicked' (Click-Island $RepeatX $ButtonY)
    Check 'the player is told too' (Wait-Until { (Get-Content $playerLog) -contains 'command repeat 2' } 2000)
    Start-Sleep -Milliseconds 500
    Shot '14b-shuffle-and-repeat-on'
    Check 'repeat moves to the next mode' (Click-Island $RepeatX $ButtonY)
    Check 'the player follows' (Wait-Until { (Get-Content $playerLog) -contains 'command repeat 1' } 2000)
    Check 'the shortcut can be clicked' (Click-Island $ShortcutX $ShortcutY)
    Start-Sleep -Milliseconds 700
    Shot '15-home'
    # The preset chip is lighter than the black a closed home view would leave.
    Check 'the home view really opened' ((Get-ScreenRed ($Preset25X - 30) $PresetY) -gt 20)
    Check 'a preset can be clicked' (Click-Island $Preset25X $PresetY)
    Start-Sleep -Milliseconds 900
    [void][Win]::SetCursorPos($centerX, 600)
    Start-Sleep -Milliseconds 1100
    Shot '16-timer-bubble'
    Check 'music stays in the pill with the timer beside it' ([Win]::Root($BubbleX, $PillY) -eq $hwnd)

    'Timer controls'
    Check 'the timer bubble can be clicked' (Click-Island $BubbleX $PillY)
    Start-Sleep -Milliseconds 800
    Shot '17-timer-open'
    Check 'pause can be clicked' (Click-Island $PauseX $TimerButtonY)
    Start-Sleep -Milliseconds 500
    Shot '18-timer-paused'
    Check 'cancel can be clicked' (Click-Island $CancelX $TimerButtonY)
    Start-Sleep -Milliseconds 1000
    [void][Win]::SetCursorPos($centerX, 600)
    Start-Sleep -Milliseconds 1100
    Check 'the event returns to the bubble' ([Win]::Root($BubbleX, $PillY) -eq $hwnd)
    $next = @(Get-Content $playerLog | Where-Object { $_ -eq 'command next' }).Count
    Check 'no click landed on a music button by mistake' ($next -eq 0)

    'Hiding an event'
    Check 'the event bubble can be clicked' (Click-Island $BubbleX $PillY)
    Start-Sleep -Milliseconds 800
    Shot '19-event-open'
    Check 'the hide button can be clicked' (Click-Island $centerX $ButtonY)
    [void][Win]::SetCursorPos($centerX, 600)
    Start-Sleep -Milliseconds 1100
    Check 'the event stays away' ([Win]::Root($BubbleX, $PillY) -ne $hwnd)
    Check 'music keeps the pill' ([Win]::Root($centerX - 60, $PillY) -eq $hwnd)
    Stop-Island
    Stop-Process -Id $player.Id -Force -ErrorAction SilentlyContinue

    'Timer running out'
    Start-Process -FilePath $Exe -ArgumentList ($NoSession + '--short-timers', '--local-timers')
    Start-Sleep -Milliseconds 1500
    [Win]::Press(0x5B, 0x12, 0x20)
    Start-Sleep -Milliseconds 500
    [Win]::Press(0x5B, 0x12, 0x20)
    Start-Sleep -Milliseconds 800
    Shot '19b-home-empty'
    Check 'a short timer starts' (Click-Island $Preset5X $PresetY)
    [void][Win]::SetCursorPos($centerX, 600)
    Start-Sleep -Milliseconds 900
    Shot '20-timer-pill'
    Check 'the timer shows in the pill' (Visible)
    Start-Sleep -Milliseconds 4300
    Shot '21-timer-done'
    Check 'it announces the end' (Visible)
    Start-Sleep -Milliseconds 9000
    Check 'it waits to be answered' (Visible)
    Check 'a click answers it' (Click-Island $centerX $PillY)
    [void][Win]::SetCursorPos($centerX, 600)
    Check 'then it leaves' (Wait-Until { -not (Visible) } 6000)
    Stop-Island

    'Home view from a timer'
    Start-Process -FilePath $Exe -ArgumentList ($NoSession + '--local-timers')
    Start-Sleep -Milliseconds 1500
    [Win]::Press(0x5B, 0x12, 0x20)
    Start-Sleep -Milliseconds 500
    [Win]::Press(0x5B, 0x12, 0x20)
    Start-Sleep -Milliseconds 800
    Check 'a timer starts' (Click-Island $Preset25X $PresetY)
    [void][Win]::SetCursorPos($centerX, 600)
    Start-Sleep -Milliseconds 1100
    Check 'the pill opens the timer' (Click-Island $centerX $PillY)
    Start-Sleep -Milliseconds 800
    Check 'the shortcut is there too' (Click-Island $ShortcutX $ShortcutY)
    Start-Sleep -Milliseconds 700
    Shot '22-home-over-timer'
    Check 'it opens the home view' ((Get-ScreenRed ($Preset25X - 30) $PresetY) -gt 20)
    Stop-Island

    'Notch'
    Start-Process -FilePath $Exe -ArgumentList ($NoSession + '--local-timers', '--notch')
    Start-Sleep -Milliseconds 1500
    [Win]::Press(0x5B, 0x12, 0x20)
    Start-Sleep -Milliseconds 900
    Shot '22b-notch-clock'
    $notch = (Island).ToInt64()
    Check 'it reaches the top of the screen' ([Win]::Root($centerX, 0) -eq $notch)
    Check 'beside it reaches the app below' ([Win]::Root($centerX + 140, 0) -ne $notch)
    Check 'the very top can be clicked' (Click-Island $centerX 0)
    Start-Sleep -Milliseconds 800
    Shot '22c-notch-home'
    Check 'it opens the home view' ((Get-ScreenRed ($Preset25X - 30) $PresetY) -gt 20)
    Check 'the pointer on the top row keeps it open' ([Win]::Root($centerX, 0) -eq $notch)
    Stop-Island
    # Left on the top edge, the pointer would bring the clock up by itself, and
    # the next double press would then open and close the home view.
    [void][Win]::SetCursorPos($centerX, 600)

    'Calendar out of reach'
    Start-Process -FilePath $Exe -ArgumentList ($NoSession + '--local-timers', '--calendar-file', 'no-such-file.ics')
    Start-Sleep -Milliseconds 1500
    [Win]::Press(0x5B, 0x12, 0x20)
    Start-Sleep -Milliseconds 500
    [Win]::Press(0x5B, 0x12, 0x20)
    Start-Sleep -Milliseconds 800
    Shot '23-home-calendar-failed'
    Stop-Island

    'Windows focus session'
    $sessions = [Windows.UI.Shell.FocusSessionManager, Windows.UI.Shell, ContentType = WindowsRuntime]::GetDefault()
    if ($sessions.IsFocusActive) {
        '  skipped, a real session is running'
    } else {
        Start-Process -FilePath $Exe -ArgumentList $NoSession
        Start-Sleep -Milliseconds 1500
        [Win]::Press(0x5B, 0x12, 0x20)
        Start-Sleep -Milliseconds 500
        [Win]::Press(0x5B, 0x12, 0x20)
        Start-Sleep -Milliseconds 800
        Check 'the timer tab can be clicked' (Click-Island $TimerTabX $TabY)
        Start-Sleep -Milliseconds 300
        Shot '24-home-timer-tab'
        Check 'it lights up' ((Get-ScreenRed $TimerChipX $TabY) -gt 20)
        Stop-Island

        Start-Process -FilePath $Exe -ArgumentList $NoSession
        Start-Sleep -Milliseconds 1500
        [Win]::Press(0x5B, 0x12, 0x20)
        Start-Sleep -Milliseconds 500
        [Win]::Press(0x5B, 0x12, 0x20)
        Start-Sleep -Milliseconds 800
        Check 'the choice is remembered' ((Get-ScreenRed $TimerChipX $TabY) -gt 20)
        Check 'the focus tab can be clicked' (Click-Island $FocusTabX $TabY)
        Start-Sleep -Milliseconds 300
        Shot '25-home-focus'
        Check 'a preset starts a real Windows session' ((Click-Island $Preset5X $PresetY) -and (Wait-Until { $sessions.IsFocusActive } 12000))
        Check 'it runs for the length that was asked' (Wait-Until { $left = Get-FocusLeft; $left -gt 270 -and $left -le 300 } 8000)
        [void][Win]::SetCursorPos($centerX, 600)
        Start-Sleep -Milliseconds 1800
        Shot '26-focus-pill'
        Check 'the pill can be clicked' (Click-Island $centerX $PillY)
        Start-Sleep -Milliseconds 800
        Shot '27-focus-open'
        # A clock pill would open the home view instead, with its lighter chips.
        Check 'the island follows it, not just the clock' ((Get-ScreenRed ($Preset25X - 30) $PresetY) -lt 20)
        Check 'pause can be clicked' (Click-Island $PauseX $TimerButtonY)
        Start-Sleep -Milliseconds 150
        Shot '28-focus-paused-at-once'
        Check 'it pauses in Windows too' (Wait-Until { -not $sessions.IsFocusActive } 4000)
        Start-Sleep -Milliseconds 1200
        Shot '29-focus-paused'
        Check 'a paused session stays on the island' (Visible)
        Check 'resume can be clicked' (Click-Island $PauseX $TimerButtonY)
        Check 'it resumes in Windows too' (Wait-Until { $sessions.IsFocusActive } 4000)
        Start-Sleep -Milliseconds 1000
        Check 'the end button can be clicked' (Click-Island $CancelX $TimerButtonY)
        Check 'it ends in Windows too' (Wait-Until { -not $sessions.IsFocusActive } 4000)
        [void][Win]::SetCursorPos($centerX, 600)
        Check 'the island lets go of it' (Wait-Until { -not (Visible) } 6000)
        Stop-Island
    }

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
