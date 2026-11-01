# Creates the silent track and cover the test player needs. Dot-source it, then
# call New-MediaFixture. The test player is the only media the scripts touch:
# the app always runs with --only-session, so a real player is never controlled.

$TestPlayerExe = "$PSScriptRoot\..\spikes\test_player\target\release\test_player.exe"
$TestSession = 'test_player.exe'
# Follows nothing, so a real player cannot affect the result.
$NoSession = @('--only-session', 'no-such-session')

function New-MediaFixture([string]$Directory) {
    New-Item -ItemType Directory -Force $Directory | Out-Null

    $wav = Join-Path $Directory 'silence.wav'
    $rate = 8000; $bytes = $rate * 2 * 900
    $memory = New-Object IO.MemoryStream
    $writer = New-Object IO.BinaryWriter $memory
    $writer.Write([Text.Encoding]::ASCII.GetBytes('RIFF')); $writer.Write([int](36 + $bytes))
    $writer.Write([Text.Encoding]::ASCII.GetBytes('WAVEfmt ')); $writer.Write([int]16)
    $writer.Write([int16]1); $writer.Write([int16]1); $writer.Write([int]$rate)
    $writer.Write([int]($rate * 2)); $writer.Write([int16]2); $writer.Write([int16]16)
    $writer.Write([Text.Encoding]::ASCII.GetBytes('data')); $writer.Write([int]$bytes)
    $writer.Write((New-Object byte[] $bytes))
    [IO.File]::WriteAllBytes($wav, $memory.ToArray())

    Add-Type -AssemblyName System.Drawing
    $cover = Join-Path $Directory 'cover.png'
    $bitmap = New-Object System.Drawing.Bitmap 480, 480
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $area = New-Object System.Drawing.Rectangle 0, 0, 480, 480
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush $area, ([System.Drawing.Color]::DeepPink), ([System.Drawing.Color]::DarkBlue), 45.0
    $graphics.FillRectangle($brush, $area); $graphics.Dispose()
    $bitmap.Save($cover, [System.Drawing.Imaging.ImageFormat]::Png); $bitmap.Dispose()

    [pscustomobject]@{ Wav = $wav; Cover = $cover }
}

function Start-TestPlayer($Fixture, [int]$Seconds, [string]$Log) {
    Start-Process -FilePath $TestPlayerExe -ArgumentList "`"$($Fixture.Wav)`"", "`"$($Fixture.Cover)`"", $Seconds -NoNewWindow -PassThru -RedirectStandardOutput $Log
}

# A made-up calendar: one event soon enough to show, one later. The u with dots
# is built by code, as Windows PowerShell misreads such letters in a script file.
function New-CalendarFixture([string]$Directory) {
    $path = Join-Path $Directory 'calendar.ics'
    $format = 'yyyyMMddTHHmmssZ'
    $soon = (Get-Date).ToUniversalTime().AddMinutes(20)
    $later = $soon.AddHours(3)
    $text = @(
        'BEGIN:VCALENDAR', 'VERSION:2.0'
        'BEGIN:VEVENT', 'UID:soon', "DTSTART:$($soon.ToString($format))", "DTEND:$($soon.AddHours(1).ToString($format))"
        'SUMMARY:Deneme Dersi', "LOCATION:Kamp$([char]0xFC)s A-12\, Ornek Universitesi", 'END:VEVENT'
        'BEGIN:VEVENT', 'UID:later', "DTSTART:$($later.ToString($format))", "DTEND:$($later.AddHours(1).ToString($format))"
        'SUMMARY:Sonraki Etkinlik', 'END:VEVENT'
        'END:VCALENDAR'
    ) -join "`r`n"
    [IO.File]::WriteAllText($path, $text, (New-Object Text.UTF8Encoding $false))
    $path
}
