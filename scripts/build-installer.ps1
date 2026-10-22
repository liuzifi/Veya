param(
    [string]$Nsis = '',
    # Build for Windows 7 SP1 (x86_64-win7-windows-msvc, nightly + build-std, static CRT).
    [switch]$Win7
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$artifacts = Join-Path $root 'artifacts'
$buildDir = Join-Path $root 'target\release-package'
if ($Win7) { $buildDir = Join-Path $root 'target\release-package-win7' }
New-Item -ItemType Directory -Force -Path $artifacts | Out-Null

if ($Win7) {
    # Tier 3 target: no prebuilt std, so build it with nightly. A static CRT avoids requiring the
    # UCRT update / VC++ redistributable on the target machine.
    $win7Target = 'x86_64-win7-windows-msvc'

    # -Zbuild-std compiles the standard library from source, which needs the rust-src component
    # of the nightly toolchain. Add it when missing and fail with a clear message otherwise.
    function Get-NightlyStdLock {
        $sysroot = (& rustc +nightly --print sysroot).Trim()
        Join-Path $sysroot 'lib\rustlib\src\rust\library\Cargo.lock'
    }
    if (-not (Test-Path -LiteralPath (Get-NightlyStdLock))) {
        Write-Host 'rust-src is missing from the nightly toolchain; installing it.'
        & rustup component add rust-src --toolchain nightly
        if ($LASTEXITCODE -ne 0) { throw "rustup component add rust-src failed: $LASTEXITCODE" }
    }
    if (-not (Test-Path -LiteralPath (Get-NightlyStdLock))) {
        & rustc +nightly --version --verbose
        & rustup component list --installed --toolchain nightly
        throw "rust-src is still missing from the nightly toolchain: $(Get-NightlyStdLock)"
    }
    $previousRustflags = $env:RUSTFLAGS
    $env:RUSTFLAGS = (($previousRustflags, '-C target-feature=+crt-static') | Where-Object { $_ }) -join ' '
    try {
        & cargo +nightly build --locked --release -p veya-desktop --target $win7Target `
            '-Zbuild-std=std,panic_abort' --target-dir $buildDir --manifest-path (Join-Path $root 'Cargo.toml')
        if ($LASTEXITCODE -ne 0) { throw "cargo build (win7) failed: $LASTEXITCODE" }
    }
    finally {
        $env:RUSTFLAGS = $previousRustflags
    }
}
else {
    & cargo build --locked --release -p veya-desktop --target-dir $buildDir --manifest-path (Join-Path $root 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed: $LASTEXITCODE" }
}

if (-not $Nsis) {
    $Nsis = @(
        (Get-Command makensis.exe -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -ErrorAction SilentlyContinue),
        'C:\Program Files (x86)\NSIS\makensis.exe',
        'C:\Program Files\NSIS\makensis.exe'
    ) | Where-Object { $_ -and (Test-Path -LiteralPath $_) } | Select-Object -First 1
}
if (-not $Nsis -or -not (Test-Path -LiteralPath $Nsis)) {
    throw 'NSIS makensis.exe not found.'
}

$metadata = & cargo metadata --no-deps --format-version 1 --manifest-path (Join-Path $root 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
$version = (($metadata | ConvertFrom-Json).packages | Where-Object name -eq 'veya-desktop').version
if (-not $version) { throw 'veya-desktop version not found' }
$binary = if ($Win7) { Join-Path $buildDir "$win7Target\release\veya.exe" } else { Join-Path $buildDir 'release\veya.exe' }
if (-not (Test-Path -LiteralPath $binary)) { throw "Binary missing: $binary" }
& (Join-Path $PSScriptRoot 'assert-gui-binary.ps1') -Binary $binary
$iconHash = (Get-FileHash -LiteralPath (Join-Path $root 'icons\icon.ico') -Algorithm SHA256).Hash.ToLowerInvariant()
$iconName = "veya-icon-$($iconHash.Substring(0, 12)).ico"
& $Nsis "/DAPP_VERSION=$version" "/DAPP_BINARY=$binary" "/DAPP_ICON_NAME=$iconName" (Join-Path $root 'installer\veya.nsi')
if ($LASTEXITCODE -ne 0) { throw "makensis failed: $LASTEXITCODE" }

$installer = Join-Path $artifacts 'veya-setup.exe'
if (-not (Test-Path -LiteralPath $installer)) { throw "Installer missing: $installer" }
Write-Host "Built $installer"
