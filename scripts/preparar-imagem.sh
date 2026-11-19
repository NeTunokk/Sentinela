#!/usr/bin/env bash
# Gera o arquivo da imagem PostgreSQL 16 que viaja junto com o pacote.
#
# Uso:
#   scripts/preparar-imagem.sh [arquivo-de-saida]
#
# O padrão é postgres16.tar.gz, na raiz do repositório. O script usa a imagem
# local quando ela já existe; só faz `docker pull` uma única vez, aqui, na
# máquina de quem empacota. O Sentinela em si nunca acessa a rede.
set -euo pipefail

raiz="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$raiz"

saida="${1:-postgres16.tar.gz}"
imagem="${SENTINELA_IMAGEM:-postgres:16}"

if ! command -v docker >/dev/null 2>&1; then
  echo "erro: docker não encontrado no PATH" >&2
  exit 1
fi

if ! docker image inspect "$imagem" >/dev/null 2>&1; then
  echo "== imagem $imagem ausente; baixando uma única vez =="
  docker pull "$imagem"
fi

echo "== gerando $saida a partir de $imagem =="
temporario="$saida.tmp"
docker save "$imagem" | gzip -1 > "$temporario"
mv "$temporario" "$saida"

if command -v sha256sum >/dev/null 2>&1; then
  hash="$(sha256sum "$saida" | cut -d' ' -f1)"
else
  hash="$(shasum -a 256 "$saida" | cut -d' ' -f1)"
fi

echo "$hash  $saida" > "$saida.sha256"
tamanho="$(du -h "$saida" | cut -f1)"

echo
echo "== imagem pronta =="
echo "arquivo: $saida ($tamanho)"
echo "hash:    $hash"
echo
echo "Coloque o arquivo ao lado dos binários antes de rodar scripts/empacotar.sh."
