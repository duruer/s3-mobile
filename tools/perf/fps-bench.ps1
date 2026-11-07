# In-game fps benchmark of s3run (Windows, throttling off via fast.ps1): HUD settings, --window 2316x904,
# S3_FPS_CAP=0, S3_SAMPLE=1; averages the per-second fps of the last 40 s. Map: fixed "384-2-Artokia" (default;
# comparable between runs) or -Random (random 384 map, changes per run: noisy).
# usage: tools/perf/fps-bench.ps1 -Bin <s3run.exe> -Out <dir> [-Mt 1] [-Secs 70] [-Random] [-Settings <mods-settings.json>]
param([string]$Bin, [string]$Out, [string]$Mt = '', [int]$Secs = 70, [switch]$Random,
      [string]$Settings = '', [string]$GameDir = '')
$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path))
if (-not $GameDir) { $GameDir = Join-Path $root 'game-files\gog-s3-ultimate' }
if (Test-Path $Out) { Remove-Item -Recurse -Force $Out }
New-Item -ItemType Directory -Force $Out | Out-Null
if ($Settings) { Copy-Item $Settings (Join-Path $Out 'mods-settings.json') }
$env:S3_FPS_CAP = '0'
$env:S3_SAMPLE = '1'
$env:S3_MT = $Mt
# Main menu -> single player scenario; map picker: tab "Multi-player Map" + first map (Artokia), OK; setup OK x2.
$pick = if ($Random) { '' } else { '3000:click 181 180;3600:click 165 255;' }
$i = "1000:move 165 235;1200:click 165 235;${pick}4600:click 630 467;7000:move 730 567;7200:click 730 567;9500:move 730 567;9700:click 730 567;$(($Secs-2)*1000):shot $Out\end.png"
& (Join-Path $PSScriptRoot 'fast.ps1') $Bin --headless --game-dir $GameDir --data-dir $Out --window 2316x904 --max-seconds $Secs --inject $i | Out-File -Encoding utf8 "$Out\log.txt"
$fps = @()
foreach ($l in Get-Content "$Out\log.txt") {
  if ($l -match '\[sample\s+([0-9.]+)s\] fps:(\d+)') {
    if ([double]$Matches[1] -ge ($Secs - 40)) { $fps += [int]$Matches[2] }
  }
}
$ct = (Select-String -Path "$Out\log.txt" -Pattern 'cpu_time_s').Line
"fps avg {0:N1} (min {1} max {2}), n={3}, {4}" -f ($fps | Measure-Object -Average).Average, ($fps | Measure-Object -Minimum).Minimum, ($fps | Measure-Object -Maximum).Maximum, $fps.Count, $ct
Select-String -Path "$Out\log.txt" -Pattern '^mt: worker' | ForEach-Object { $_.Line }
