# Repair the icon resources of a `bun build --compile --windows-icon` exe.
#
#   powershell -ExecutionPolicy Bypass -File packaging\windows\fix-exe-icon.ps1 dist\superterminal.exe assets\superterminal.ico
#
# Bun 1.4.0 writes our RT_ICON entries and a numbered RT_GROUP_ICON, but leaves
# its own named group "IDI_MYICON" behind, still pointing at RT_ICON 1 (now the
# 16 px entry) while declaring it 256x256. Named groups sort before numbered
# ones in the resource directory, so Explorer and the taskbar pick the stale
# group and upscale 16 px into a blur. The numbered group's 256 px size field
# is also wrong. This deletes the named group and rewrites group 0 from the
# .ico header, in place (the .rsrc section keeps its size; .bun is untouched).
param(
  [Parameter(Mandatory = $true)][string]$Exe,
  [Parameter(Mandatory = $true)][string]$Ico
)
$ErrorActionPreference = 'Stop'
$sig = @'
[DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern IntPtr BeginUpdateResourceW(string f, bool del);
[DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode, EntryPoint="UpdateResourceW")] public static extern bool UpdateByName(IntPtr h, IntPtr type, string name, ushort lang, byte[] data, uint len);
[DllImport("kernel32.dll", SetLastError=true, EntryPoint="UpdateResourceW")] public static extern bool UpdateById(IntPtr h, IntPtr type, IntPtr name, ushort lang, byte[] data, uint len);
[DllImport("kernel32.dll", SetLastError=true, EntryPoint="EndUpdateResourceW")] public static extern bool EndUpdate(IntPtr h, bool discard);
'@
$t = Add-Type -MemberDefinition $sig -Name ResUpdate -Namespace Superterminal -PassThru
$RT_GROUP_ICON = [IntPtr]14
$LANG_EN_US = 1033

$exePath = (Resolve-Path $Exe).Path
$icoPath = (Resolve-Path $Ico).Path
[byte[]]$ico = [IO.File]::ReadAllBytes($icoPath)
$n = [BitConverter]::ToUInt16($ico, 4)
# GRPICONDIR: the .ico header, then per entry the 12 leading ICONDIRENTRY
# bytes (no file offset) followed by the RT_ICON id bun assigned (1-based, in
# .ico order).
$grp = New-Object byte[] (6 + 14 * $n)
[Array]::Copy($ico, 0, $grp, 0, 6)
for ($i = 0; $i -lt $n; $i++) {
  [Array]::Copy($ico, 6 + 16 * $i, $grp, 6 + 14 * $i, 12)
  [Array]::Copy([BitConverter]::GetBytes([uint16]($i + 1)), 0, $grp, 6 + 14 * $i + 12, 2)
}

function Begin-Update {
  $h = $t::BeginUpdateResourceW($exePath, $false)
  if ($h -eq [IntPtr]::Zero) { throw "BeginUpdateResource failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
  $h
}
# Two sessions: a failed UpdateResource (deleting a group that is already
# gone, error 87) poisons its session, so every later call reports 1359.
$h = Begin-Update
if ($t::UpdateByName($h, $RT_GROUP_ICON, 'IDI_MYICON', $LANG_EN_US, $null, 0)) {
  if (-not $t::EndUpdate($h, $false)) { throw "EndUpdateResource (delete) failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
  Write-Host "deleted stale IDI_MYICON group"
} else {
  [void]$t::EndUpdate($h, $true)
  Write-Host "IDI_MYICON group not present ($([Runtime.InteropServices.Marshal]::GetLastWin32Error())), nothing to delete"
}
$h = Begin-Update
if (-not $t::UpdateById($h, $RT_GROUP_ICON, [IntPtr]0, $LANG_EN_US, $grp, $grp.Length)) {
  throw "UpdateResource(group 0) failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
}
if (-not $t::EndUpdate($h, $false)) { throw "EndUpdateResource failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
Write-Host "icon resources rewritten: $n entries in group 0, $exePath"
