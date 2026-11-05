# Замер памяти и процессора Tg-Lite. Запустите tg-lite.exe, войдите в чат и в другом окне PowerShell выполните:
#   powershell -ExecutionPolicy Bypass -File tools\measure.ps1 -Minutes 10
# Скрипт каждые 5 секунд записывает «закрытый рабочий набор» (как в Диспетчере задач → Подробности) и нагрузку на процессор,
# затем печатает строку для таблицы в README.
# Measure Tg-Lite memory and CPU. Start tg-lite.exe, open a chat, then run this script in another PowerShell window.
param(
    [int]$Minutes = 10,
    [int]$IntervalSec = 5,
    [string]$Name = "tg-lite"
)

$proc = Get-Process -Name $Name -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) {
    Write-Host "Процесс $Name не найден. Запустите tg-lite.exe и повторите. / Process $Name not found. Start tg-lite.exe first."
    exit 1
}

$cores = [Environment]::ProcessorCount
$exe = $proc.Path
$sizeKb = if ($exe) { [math]::Round((Get-Item $exe).Length / 1KB) } else { 0 }
$os = (Get-CimInstance Win32_OperatingSystem).Caption
$samples = @()
$prevCpu = $proc.TotalProcessorTime.TotalSeconds
$prevTime = Get-Date
$end = $prevTime.AddMinutes($Minutes)
Write-Host "Замер $Minutes мин, процесс $($proc.Id), $sizeKb КБ. Ctrl+C — остановить раньше."

while ((Get-Date) -lt $end) {
    Start-Sleep -Seconds $IntervalSec
    $p = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
    if (-not $p) { Write-Host "Процесс завершился."; break }
    $perf = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter "IDProcess=$($proc.Id)" -ErrorAction SilentlyContinue
    $mem = if ($perf) { [double]$perf.WorkingSetPrivate } else { [double]$p.PrivateMemorySize64 }
    $now = Get-Date
    $cpuNow = $p.TotalProcessorTime.TotalSeconds
    $cpu = ($cpuNow - $prevCpu) / (($now - $prevTime).TotalSeconds * $cores) * 100
    $prevCpu = $cpuNow
    $prevTime = $now
    $samples += [pscustomobject]@{ Time = $now.ToString("HH:mm:ss"); MemMB = [math]::Round($mem / 1MB, 2); CpuPct = [math]::Round($cpu, 2) }
    Write-Host ("{0}  память {1} МБ  процессор {2} %" -f $samples[-1].Time, $samples[-1].MemMB, $samples[-1].CpuPct)
}

if ($samples.Count -eq 0) { exit 1 }
$csv = Join-Path (Split-Path $PSCommandPath -Parent) "measure-result.csv"
$samples | Export-Csv -Path $csv -NoTypeInformation -Encoding UTF8
$memAvg = ($samples | Measure-Object MemMB -Average).Average
$memMax = ($samples | Measure-Object MemMB -Maximum).Maximum
$cpuAvg = ($samples | Measure-Object CpuPct -Average).Average
$ver = if ($exe) { (Get-Item $exe).VersionInfo.ProductVersion } else { "?" }

Write-Host ""
Write-Host "Итог / Summary"
Write-Host ("  Windows: {0}" -f $os)
Write-Host ("  Tg-Lite: {0}, exe {1} КБ" -f $ver, $sizeKb)
Write-Host ("  Память (закрытый рабочий набор): средняя {0:N1} МБ, максимум {1:N1} МБ" -f $memAvg, $memMax)
Write-Host ("  Процессор: в среднем {0:N1} % (от всех ядер)" -f $cpuAvg)
Write-Host ("  Сырые данные: {0}" -f $csv)
Write-Host ""
Write-Host "Строка для README / README row:"
Write-Host ("| measured on {0} | {1:N1} MB avg, {2:N1} MB max | {3:N1} % | {4} KB |" -f $ver, $memAvg, $memMax, $cpuAvg, $sizeKb)
