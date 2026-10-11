$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$cargoCommand = Get-Command cargo -ErrorAction SilentlyContinue
$cargo = if ($cargoCommand) { $cargoCommand.Source } else { Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe' }
$env:PATH = "$(Split-Path -Parent $cargo);$env:PATH"
& $cargo --config (Join-Path $root '.cargo\config.toml') test --manifest-path (Join-Path $root 'Cargo.toml') --offline
if ($LASTEXITCODE -ne 0) { throw 'Tests failed.' }
& (Join-Path $root 'build.ps1')
$report = Join-Path $root 'bin\verification.txt'
if (Test-Path -LiteralPath $report) { Remove-Item -LiteralPath $report }
$process = Start-Process -FilePath (Join-Path $root 'bin\WheelFlow.exe') -ArgumentList @('--verify', ('"' + $report + '"')) -WindowStyle Hidden -PassThru
if (-not $process.WaitForExit(30000)) { $process.Kill(); throw 'Native input verification timed out.' }
$process.Refresh()
if (Test-Path -LiteralPath $report) { Get-Content -LiteralPath $report }
if ($process.ExitCode -ne 0) { throw "Native input verification failed (exit $($process.ExitCode))." }
