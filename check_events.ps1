$events = Get-WinEvent -FilterHashtable @{LogName='Application'; Id=1000,1001,1002} -MaxEvents 10 -ErrorAction SilentlyContinue
$hits = $events | Where-Object { $_.Message -match 'NetTool' }
if (-not $hits) { Write-Output "no-nettool-events"; exit 0 }
foreach ($e in $hits) {
    $lines = ($e.Message -split "`n") | Select-Object -First 6
    Write-Output ($e.TimeCreated.ToString("HH:mm:ss") + " [" + $e.Id + "] " + ($lines -join " | "))
}
