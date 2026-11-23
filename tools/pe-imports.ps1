param([string]$Path = 'target\release\wisp.exe')

# Reads the PE header directly: subsystem + the real import table (DLL names).
# grep-ing for "vcruntime140.dll" is not proof; the import directory is.

$bytes = [System.IO.File]::ReadAllBytes((Resolve-Path $Path).Path)

$peOff = [System.BitConverter]::ToInt32($bytes, 60)
$numSections = [System.BitConverter]::ToUInt16($bytes, $peOff + 6)
$sizeOptional = [System.BitConverter]::ToUInt16($bytes, $peOff + 20)
$optOff = $peOff + 24

$magic = [System.BitConverter]::ToUInt16($bytes, $optOff)
$subsystem = [System.BitConverter]::ToUInt16($bytes, $optOff + 68)

if ($magic -eq 0x20b) { $ddOff = $optOff + 112 } else { $ddOff = $optOff + 96 }
$importRva = [System.BitConverter]::ToUInt32($bytes, $ddOff + 8)

$secOff = $optOff + $sizeOptional
$sections = @()
for ($i = 0; $i -lt $numSections; $i++) {
    $s = $secOff + ($i * 40)
    $sections += [PSCustomObject]@{
        VirtualAddr  = [System.BitConverter]::ToUInt32($bytes, $s + 12)
        VirtualSize  = [System.BitConverter]::ToUInt32($bytes, $s + 8)
        RawSize      = [System.BitConverter]::ToUInt32($bytes, $s + 16)
        RawPtr       = [System.BitConverter]::ToUInt32($bytes, $s + 20)
    }
}

function Get-FileOffset([uint32]$rva) {
    foreach ($sec in $sections) {
        $span = [Math]::Max($sec.VirtualSize, $sec.RawSize)
        if ($rva -ge $sec.VirtualAddr -and $rva -lt ($sec.VirtualAddr + $span)) {
            return [int]($sec.RawPtr + ($rva - $sec.VirtualAddr))
        }
    }
    throw "RVA $rva is not inside any section"
}

function Get-AsciiString([int]$offset) {
    $sb = New-Object System.Text.StringBuilder
    while ($bytes[$offset] -ne 0) {
        [void]$sb.Append([Char]$bytes[$offset])
        $offset = $offset + 1
    }
    return $sb.ToString()
}

$dlls = @()
$desc = $importRva
for ($i = 0; $i -lt 128; $i++) {
    $entry = Get-FileOffset ([uint32]($desc + ($i * 20)))
    $nameRva = [System.BitConverter]::ToUInt32($bytes, $entry + 12)
    if ($nameRva -eq 0) { break }
    $dlls += Get-AsciiString (Get-FileOffset $nameRva)
}

$names = @{ 2 = 'GUI (windows subsystem)'; 3 = 'Console (CUI)' }
if ($names.ContainsKey([int]$subsystem)) { $label = $names[[int]$subsystem] } else { $label = "code $subsystem" }

Write-Output ("file      : {0}" -f $Path)
Write-Output ("size      : {0} KB" -f ([int]((Get-Item $Path).Length / 1KB)))
Write-Output ("subsystem : {0} ({1})" -f $subsystem, $label)
Write-Output "imports   :"
foreach ($d in ($dlls | Sort-Object)) { Write-Output ("  {0}" -f $d) }

$runtime = @($dlls | Where-Object { $_ -match 'vcruntime|msvcp|concrt|ucrtbase' })
if ($runtime.Count -eq 0) {
    Write-Output "verdict   : no CRT DLL import (static CRT)"
} else {
    Write-Output ("verdict   : needs {0}" -f ($runtime -join ', '))
}
