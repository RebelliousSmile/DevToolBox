param([ValidateSet("nsis")][string]$Format = "nsis")
$ErrorActionPreference = "Stop"
python scripts/verify-package-config.py
cargo build --release --locked
$installed = cargo packager --version 2>$null
if ($LASTEXITCODE -ne 0 -or $installed -notmatch "0\.11\.8") {
    throw "cargo-packager 0.11.8 est requis: cargo install cargo-packager --version 0.11.8 --locked"
}
cargo packager --release --formats $Format
Get-ChildItem dist -File | Select-Object Name, Length
