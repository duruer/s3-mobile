# Run a command with Windows power throttling (EcoQoS) disabled and high priority; prints "cpu_time_s N" and its
# stdout/stderr. Processes started from an agent shell are otherwise throttled 2-4x (docs/porting/performance.md).
# usage: fast.ps1 <exe> <args...>
param([string]$exe, [Parameter(ValueFromRemainingArguments=$true)][string[]]$rest)
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class PT {
  [StructLayout(LayoutKind.Sequential)] public struct STATE { public uint Version; public uint ControlMask; public uint StateMask; }
  [DllImport("kernel32.dll", SetLastError=true)] public static extern bool SetProcessInformation(IntPtr h, int cls, ref STATE s, uint size);
  public static bool Off(IntPtr h) { var s = new STATE { Version = 1, ControlMask = 1 | 4, StateMask = 0 }; return SetProcessInformation(h, 4, ref s, 12); }
}
'@
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $exe
$psi.Arguments = ($rest | ForEach-Object { if ($_ -match '[\s;"]') { '"' + ($_ -replace '"','\"') + '"' } else { $_ } }) -join ' '
$psi.UseShellExecute = $false
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$p = [System.Diagnostics.Process]::Start($psi)
[void][PT]::Off($p.Handle)
$p.PriorityClass = 'High'
$out = $p.StandardOutput.ReadToEndAsync()
$err = $p.StandardError.ReadToEndAsync()
$p.WaitForExit()
"cpu_time_s {0:N2}" -f $p.TotalProcessorTime.TotalSeconds
$out.Result
$err.Result
