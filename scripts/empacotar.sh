#!/usr/bin/env bash
# Monta um pacote distribuível do Sentinela, para entregar a quem não tem Rust.
#
# Uso:
#   scripts/empacotar.sh [--sem-build] [--versao X.Y.Z]
#
#   --sem-build   não compila; usa os binários que já estão em target/release
#   --versao      versão do pacote (padrão: a declarada em Cargo.toml)
#
# O resultado fica em dist/:
#   sentinela-<versao>-<sistema>-<arquitetura>.tar.gz
#   sentinela-<versao>-<sistema>-<arquitetura>.tar.gz.sha256
set -euo pipefail

raiz="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$raiz"

sem_build=0
versao=""
while [ $# -gt 0 ]; do
  case "$1" in
    --sem-build) sem_build=1 ;;
    --versao)
      shift
      versao="${1:-}"
      if [ -z "$versao" ]; then
        echo "erro: --versao precisa de um valor" >&2
        exit 2
      fi
      ;;
    -h|--help)
      cat <<'TXT'
Monta um pacote distribuível do Sentinela, para entregar a quem não tem Rust.

Uso:
  scripts/empacotar.sh [--sem-build] [--versao X.Y.Z]

  --sem-build   não compila; usa os binários que já estão em target/release
  --versao      versão do pacote (padrão: a declarada em Cargo.toml)

O resultado fica em dist/:
  sentinela-<versao>-<sistema>-<arquitetura>.tar.gz
  sentinela-<versao>-<sistema>-<arquitetura>.tar.gz.sha256
TXT
      exit 0
      ;;
    *)
      echo "erro: argumento desconhecido: $1" >&2
      exit 2
      ;;
  esac
  shift
done

if [ -z "$versao" ]; then
  versao="$(grep -m1 -A6 '^\[workspace.package\]' Cargo.toml \
    | grep -m1 '^version' \
    | cut -d'"' -f2)"
fi

if [ -z "$versao" ]; then
  echo "erro: não encontrei a versão em Cargo.toml" >&2
  exit 1
fi

sistema="$(uname -s)"
case "$sistema" in
  Linux) so="linux" ;;
  Darwin) so="macos" ;;
  *)
    echo "erro: sistema não suportado por este script: $sistema" >&2
    exit 1
    ;;
esac

maquina="$(uname -m)"
case "$maquina" in
  x86_64|amd64) arq="x86_64" ;;
  aarch64|arm64) arq="aarch64" ;;
  *) arq="$maquina" ;;
esac

alvo="${CARGO_TARGET_DIR:-target}/release"
nome="sentinela-${versao}-${so}-${arq}"

if [ "$sem_build" -eq 0 ]; then
  echo "== compilando (release) =="
  cargo build --locked --release --workspace
fi

for bin in sentinela sentinela-server; do
  if [ ! -f "$alvo/$bin" ]; then
    echo "erro: binário ausente: $alvo/$bin" >&2
    echo "      rode sem --sem-build para compilar primeiro" >&2
    exit 1
  fi
done

rm -rf "dist/$nome" "dist/$nome.tar.gz" "dist/$nome.tar.gz.sha256"
mkdir -p "dist/$nome"

cp "$alvo/sentinela" "$alvo/sentinela-server" "dist/$nome/"
chmod 0755 "dist/$nome/sentinela" "dist/$nome/sentinela-server"
cp README.md "dist/$nome/"
cp scripts/COMO-USAR.txt "dist/$nome/"
if [ -f LICENSE ]; then
  cp LICENSE "dist/$nome/"
fi

tar -C dist -czf "dist/$nome.tar.gz" "$nome"

if command -v sha256sum >/dev/null 2>&1; then
  ( cd dist && sha256sum "$nome.tar.gz" > "$nome.tar.gz.sha256" )
else
  ( cd dist && shasum -a 256 "$nome.tar.gz" > "$nome.tar.gz.sha256" )
fi

tamanho="$(du -h "dist/$nome.tar.gz" | cut -f1)"
echo
echo "== pacote pronto =="
echo "arquivo: dist/$nome.tar.gz ($tamanho)"
echo "hash:    $(cut -d' ' -f1 "dist/$nome.tar.gz.sha256")"
echo
echo "conteúdo:"
tar -tzf "dist/$nome.tar.gz" | sed 's/^/  /'
