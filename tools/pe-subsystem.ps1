param([string]$Path = '')

function Get-Subsystem([string]$file) {
    $bytes = [System.IO.File]::ReadAllBytes($file)
    $peOffset = [System.BitConverter]::ToInt32($bytes, 60)
    # PE signature(4) + COFF header(20) + optional header; Subsystem sits at +68 in PE32+.
    $subsystem = [System.BitConverter]::ToUInt16($bytes, ($peOffset + 24 + 68))
    $names = @{ 2 = 'GUI (windows)'; 3 = 'Console (CUI)' }
    if ($names.ContainsKey([int]$subsystem)) { return "$subsystem -> " + $names[[int]$subsystem] }
    return "$subsystem -> ?"
}

if ($Path) { Write-Output ("{0} : {1}" -f $Path, (Get-Subsystem $Path)) }
foreach ($p in @('target\debug\wisp.exe', 'target\release\wisp.exe')) {
    if (Test-Path $p) {
        $kb = [int]((Get-Item $p).Length / 1KB)
        Write-Output ("{0} : {1} : {2} KB" -f $p, (Get-Subsystem $p), $kb)
    }
}
