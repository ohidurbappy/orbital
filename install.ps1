# orbital installer for Windows (PowerShell).
#
# Downloads the latest release binary and installs it as `orbital.exe`, adding
# it to your user PATH. Usage:
#
#   irm https://raw.githubusercontent.com/ohidurbappy/orbital/main/install.ps1 | iex
#
# Override the install directory with $env:ORBITAL_INSTALL_DIR before running.
$ErrorActionPreference = 'Stop'

$repo = 'ohidurbappy/orbital'
$asset = 'orbital-windows-x64.exe.gz'
$url = "https://github.com/$repo/releases/latest/download/$asset"

# --- choose install directory ------------------------------------------------
$dir = if ($env:ORBITAL_INSTALL_DIR) { $env:ORBITAL_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'orbital\bin' }
New-Item -ItemType Directory -Force -Path $dir | Out-Null

$gzPath = Join-Path $env:TEMP 'orbital.exe.gz'
$exePath = Join-Path $dir 'orbital.exe'

# --- download ----------------------------------------------------------------
Write-Host "Downloading $asset ..."
Invoke-WebRequest -Uri $url -OutFile $gzPath -UseBasicParsing

# --- decompress gzip ---------------------------------------------------------
$inStream = [System.IO.File]::OpenRead($gzPath)
$outStream = [System.IO.File]::Create($exePath)
$gzip = New-Object System.IO.Compression.GzipStream($inStream, [System.IO.Compression.CompressionMode]::Decompress)
try {
  $gzip.CopyTo($outStream)
} finally {
  $gzip.Dispose(); $outStream.Dispose(); $inStream.Dispose()
}
Remove-Item $gzPath -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "Installed orbital to $exePath"

# --- add to user PATH if missing ---------------------------------------------
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $dir) {
  [Environment]::SetEnvironmentVariable('Path', "$userPath;$dir", 'User')
  Write-Host "Added $dir to your user PATH — restart your terminal to use 'orbital'."
}

Write-Host ""
Write-Host "Run: orbital --help"
