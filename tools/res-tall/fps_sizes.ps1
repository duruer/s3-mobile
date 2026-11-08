# res-tall: uncapped fps per forced render size (HUD, window 2316x904), EcoQoS off via tools/perf/fast.ps1.
# usage: fps_sizes.ps1 <s3run.exe> <game dir> <data dir (with mods-settings.json)> <out dir> <WxH> [<WxH> ...]
# Writes <out>/fps_<WxH>.txt (S3_SAMPLE lines) and prints the median fps of the samples 62..85 s.
param([string]$exe, [string]$game, [string]$data, [string]$out, [Parameter(ValueFromRemainingArguments=$true)][string[]]$sizes)
$fast = Join-Path $PSScriptRoot '..\perf\fast.ps1'
New-Item -ItemType Directory -Force $out | Out-Null
$inj = '1000:move 165 235;1200:click 165 235;4000:move 630 467;4200:click 630 467;7000:move 730 567;7200:click 730 567;9500:move 730 567;9700:click 730 567'
foreach ($s in $sizes) {
  $env:S3_RES = $s; $env:S3_FPS_CAP = '0'; $env:S3_SAMPLE = '1'
  $f = Join-Path $out "fps_$s.txt"
  & $fast $exe --headless --game-dir $game --data-dir $data --display native --window 2316x904 --max-seconds 88 --inject $inj | Out-File -Encoding utf8 $f
  $v = @(Select-String -Path $f -Pattern 'fps:(\d+)' | ForEach-Object { [int]$_.Matches[0].Groups[1].Value })
  $tail = if ($v.Count -gt 26) { $v[($v.Count - 26)..($v.Count - 3)] } else { $v }
  $sorted = $tail | Sort-Object
  $med = if ($sorted.Count) { $sorted[[int]($sorted.Count / 2)] } else { -1 }
  "{0}: median fps {1} (samples {2})" -f $s, $med, ($tail -join ',')
}
