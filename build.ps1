$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$cargoCommand = Get-Command cargo -ErrorAction SilentlyContinue
$cargo = if ($cargoCommand) { $cargoCommand.Source } else { Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe' }
if (-not (Test-Path -LiteralPath $cargo)) { throw 'Rust MSVC toolchain is required. Install it with rustup, then reopen the terminal.' }
$cargoBin = Split-Path -Parent $cargo
$env:PATH = "$cargoBin;$env:PATH"
& $cargo --config (Join-Path $root '.cargo\config.toml') build --manifest-path (Join-Path $root 'Cargo.toml') --release --offline
if ($LASTEXITCODE -ne 0) { throw 'Build failed.' }
$out = Join-Path $root 'bin'
New-Item -ItemType Directory -Force -Path $out | Out-Null
$target = Join-Path $out 'WheelFlow.exe'
Copy-Item -LiteralPath (Join-Path $root 'target\release\WheelFlow.exe') -Destination $target -Force
Write-Host "Built $out\WheelFlow.exe"
