param(
    [string]$Tag = $env:GITHUB_REF_NAME,
    [string]$Repository = $env:GITHUB_REPOSITORY,
    [string]$RunId = $env:GITHUB_RUN_ID
)

$ErrorActionPreference = 'Stop'
if ($Tag -cnotmatch '^v[0-9]+\.[0-9]+\.[0-9]+$') { throw "Invalid release tag: $Tag" }
$root = Split-Path $PSScriptRoot -Parent
$notesPath = Join-Path $root "docs\releases\$Tag.md"
$installer = Join-Path $root 'artifacts\veya-setup.exe'
if (-not (Test-Path -LiteralPath $notesPath)) { throw "Release notes missing: $notesPath" }
if (-not (Test-Path -LiteralPath $installer)) { throw "Installer missing: $installer" }

$sha256 = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
$checksumPath = Join-Path $root 'artifacts\veya-setup.exe.sha256'
Set-Content -LiteralPath $checksumPath -Value "$sha256 *veya-setup.exe" -Encoding utf8
$sizeMiB = [math]::Round((Get-Item -LiteralPath $installer).Length / 1MB, 2)

# Windows 7 installer (optional, built with nightly + x86_64-win7-windows-msvc)
$win7Installer = Join-Path $root 'artifacts\veya-win7-setup.exe'
$win7Info = $null
if (Test-Path -LiteralPath $win7Installer) {
    $win7Sha = (Get-FileHash -LiteralPath $win7Installer -Algorithm SHA256).Hash.ToLowerInvariant()
    $win7ChecksumPath = Join-Path $root 'artifacts\veya-win7-setup.exe.sha256'
    Set-Content -LiteralPath $win7ChecksumPath -Value "$win7Sha *veya-win7-setup.exe" -Encoding utf8
    $win7SizeMiB = [math]::Round((Get-Item -LiteralPath $win7Installer).Length / 1MB, 2)
    $win7Info = @{ Sha = $win7Sha; Size = $win7SizeMiB }
}
$commit = (& git rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve HEAD' }
$curated = Get-Content -Raw -Encoding UTF8 -LiteralPath $notesPath
if ([string]::IsNullOrWhiteSpace($curated)) { throw "Release notes are empty: $notesPath" }
$runLink = if ($Repository -and $RunId) { "https://github.com/$Repository/actions/runs/$RunId" } else { 'local build' }
$bodyLines = @(
    $curated.TrimEnd()
    ''
    '## 构建与下载'
    ''
    "- 版本：``$Tag``"
    "- 提交：``$commit``"
    "- Windows 构建：$runLink"
    '- 验证：格式检查、workspace 测试、release 编译、NSIS 安装包构建'
    "- 安装包：``veya-setup.exe``，$sizeMiB MiB"
    "- SHA-256：``$sha256``"
)
if ($win7Info) {
    $bodyLines += @(
        ''
        '### Windows 7 SP1 (x64)'
        ''
        '- 使用 `x86_64-win7-windows-msvc` 目标（nightly + build-std + 静态 CRT）构建，兼容 Windows 7 SP1。'
        '- DPI 检测在运行时动态加载 `shcore.dll`，缺失时回退到系统 DPI / 96。'
        "- 安装包：``veya-win7-setup.exe``，$($win7Info.Size) MiB"
        "- SHA-256：``$($win7Info.Sha)``"
    )
}
$bodyLines += @(
    ''
    '安装器覆盖旧版程序并保留 `%APPDATA%\Veya` 中的用户数据。'
)
$body = $bodyLines -join "`n"
Set-Content -LiteralPath (Join-Path $root 'release-body.md') -Value $body -Encoding utf8
Write-Host "Release body and checksum ready for $Tag"
