<#
.SYNOPSIS
  Gera o arquivo da imagem PostgreSQL 16 que viaja junto com o pacote.

.DESCRIPTION
  Usa a imagem local quando já existe; só faz `docker pull` uma única vez, aqui,
  na máquina de quem empacota. O Sentinela em si nunca acessa a rede.

  No Windows o arquivo é gerado sem compressão (postgres16.tar), porque o
  PowerShell não comprime bem arquivos deste tamanho. O Sentinela aceita tanto
  .tar quanto .tar.gz, e o zip final do pacote comprime o conteúdo de qualquer
  forma.

.PARAMETER Saida
  Nome do arquivo gerado. Padrão: postgres16.tar.

.PARAMETER Imagem
  Imagem de origem. Padrão: postgres:16 (ou a variável SENTINELA_IMAGEM).

.EXAMPLE
  .\scripts\preparar-imagem.ps1
#>
[CmdletBinding()]
param(
  [string]$Saida = "postgres16.tar",
  [string]$Imagem = $(if ($env:SENTINELA_IMAGEM) { $env:SENTINELA_IMAGEM } else { "postgres:16" })
)

$ErrorActionPreference = "Stop"
$raiz = Split-Path -Parent $PSScriptRoot
Set-Location $raiz

if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
  throw "docker não encontrado no PATH"
}

docker image inspect $Imagem *> $null
if ($LASTEXITCODE -ne 0) {
  Write-Host "== imagem $Imagem ausente; baixando uma única vez =="
  docker pull $Imagem
  if ($LASTEXITCODE -ne 0) { throw "docker pull falhou (código $LASTEXITCODE)" }
}

Write-Host "== gerando $Saida a partir de $Imagem =="
$temporario = "$Saida.tmp"
Remove-Item $temporario -Force -ErrorAction SilentlyContinue
docker save $Imagem -o $temporario
if ($LASTEXITCODE -ne 0) { throw "docker save falhou (código $LASTEXITCODE)" }
Move-Item $temporario $Saida -Force

$hash = (Get-FileHash -Path $Saida -Algorithm SHA256).Hash.ToLower()
"$hash  $Saida" | Set-Content -Path "$Saida.sha256" -Encoding ascii -NoNewline
$tamanho = [math]::Round((Get-Item $Saida).Length / 1MB, 1)

Write-Host ""
Write-Host "== imagem pronta =="
Write-Host "arquivo: $Saida ($tamanho MB)"
Write-Host "hash:    $hash"
Write-Host ""
Write-Host "Coloque o arquivo ao lado dos binários antes de rodar scripts/empacotar.ps1."
