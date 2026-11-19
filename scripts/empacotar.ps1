<#
.SYNOPSIS
  Monta um pacote distribuível do Sentinela para Windows.

.DESCRIPTION
  Gera dist/sentinela-<versao>-windows-<arquitetura>.zip com os dois
  executáveis, o README, o COMO-USAR.txt e o arquivo .sha256 correspondente.
  Serve para entregar o programa a quem não tem Rust instalado.

.PARAMETER SemBuild
  Não compila; usa os binários que já estão em target\release.

.PARAMETER Versao
  Versão do pacote. Padrão: a declarada em Cargo.toml.

.EXAMPLE
  .\scripts\empacotar.ps1

.EXAMPLE
  .\scripts\empacotar.ps1 -SemBuild -Versao 0.1.0
#>
[CmdletBinding()]
param(
  [switch]$SemBuild,
  [string]$Versao
)

$ErrorActionPreference = "Stop"
$raiz = Split-Path -Parent $PSScriptRoot
Set-Location $raiz

if (-not $Versao) {
  $linha = Select-String -Path "Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
  if (-not $linha) { throw "não encontrei a versão em Cargo.toml" }
  $Versao = $linha.Matches[0].Groups[1].Value
}

switch ($env:PROCESSOR_ARCHITECTURE) {
  "AMD64" { $arq = "x86_64" }
  "ARM64" { $arq = "aarch64" }
  default { $arq = ($env:PROCESSOR_ARCHITECTURE | ForEach-Object { $_.ToLower() }) }
}

$alvo = if ($env:CARGO_TARGET_DIR) { Join-Path $env:CARGO_TARGET_DIR "release" } else { Join-Path "target" "release" }
$nome = "sentinela-$Versao-windows-$arq"

if (-not $SemBuild) {
  Write-Host "== compilando (release) =="
  cargo build --locked --release --workspace
  if ($LASTEXITCODE -ne 0) { throw "cargo build falhou (código $LASTEXITCODE)" }
}

foreach ($bin in @("sentinela.exe", "sentinela-server.exe")) {
  $caminho = Join-Path $alvo $bin
  if (-not (Test-Path $caminho)) {
    throw "binário ausente: $caminho (rode sem -SemBuild para compilar primeiro)"
  }
}

$destino = Join-Path "dist" $nome
$zip = Join-Path "dist" "$nome.zip"

Remove-Item $destino -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item $zip -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $destino | Out-Null

Copy-Item (Join-Path $alvo "sentinela.exe") $destino
Copy-Item (Join-Path $alvo "sentinela-server.exe") $destino
Copy-Item "README.md" $destino
Copy-Item (Join-Path "scripts" "COMO-USAR.txt") $destino
if (Test-Path "LICENSE") { Copy-Item "LICENSE" $destino }

Compress-Archive -Path (Join-Path $destino "*") -DestinationPath $zip -CompressionLevel Optimal -Force

$hash = (Get-FileHash -Path $zip -Algorithm SHA256).Hash.ToLower()
"$hash  $nome.zip" | Set-Content -Path "$zip.sha256" -Encoding ascii -NoNewline

$tamanho = [math]::Round((Get-Item $zip).Length / 1MB, 1)
Write-Host ""
Write-Host "== pacote pronto =="
Write-Host "arquivo: $zip ($tamanho MB)"
Write-Host "hash:    $hash"
Write-Host ""
Write-Host "conteúdo:"
Get-ChildItem $destino | ForEach-Object { Write-Host ("  " + $_.Name) }
