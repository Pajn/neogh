$REPO = "Pajn/neogh"
$LUA_DIR = "lua"

$OS = $PSVersionTable.OS
if ($IsWindows) {
    $TARGET = "x86_64-pc-windows-msvc"
    $EXT = "dll"
} else {
    Write-Error "This script is for Windows. Please use install.sh for Unix-like systems."
    exit 1
}

$BINARY_NAME = "neogh-$TARGET.$EXT"
$URL = "https://github.com/$REPO/releases/download/nightly/$BINARY_NAME"

Write-Host "Downloading $BINARY_NAME from $URL..."

if (-not (Test-Path $LUA_DIR)) {
    New-Item -ItemType Directory -Path $LUA_DIR
}

Invoke-WebRequest -Uri $URL -OutFile "$LUA_DIR/neogh.dll"

Write-Host "Successfully installed neogh.dll"
