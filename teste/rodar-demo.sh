#!/usr/bin/env bash
set -euo pipefail

MODO="ok"
MANTER=0
for argumento in "$@"; do
  case "$argumento" in
    --falha) MODO="falha" ;;
    --manter) MANTER=1 ;;
    -h|--help)
      cat <<'AJUDA'
Uso: ./teste/rodar-demo.sh [--falha] [--manter]

  (sem opções)  espera exit 0: restore ok e todos os testes passam
  --falha       espera exit 1: inclui um teste que falha de propósito
  --manter      mantém o container do sandbox para depuração

Variáveis opcionais:
  SENTINELA_TESTE_DIR    diretório de trabalho (padrão: teste/.work)
  SENTINELA_TESTE_IMAGE  imagem PostgreSQL (padrão: postgres:16)
AJUDA
      exit 0
      ;;
    *)
      echo "erro: argumento desconhecido: $argumento" >&2
      exit 2
      ;;
  esac
done

RAIZ="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TRABALHO="${SENTINELA_TESTE_DIR:-$RAIZ/teste/.work}"
BINARIO="$RAIZ/target/release/sentinela"
IMAGEM="${SENTINELA_TESTE_IMAGE:-postgres:16}"
ORIGEM="sentinela-teste-origem-$$"

precisa() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "erro: '$1' não encontrado no PATH" >&2
    exit 1
  }
}

precisa docker

if [ ! -x "$BINARIO" ]; then
  precisa cargo
  echo "==> compilando o Sentinela (release)"
  (cd "$RAIZ" && cargo build --release)
fi
[ -x "$BINARIO" ] || {
  echo "erro: binário não encontrado em $BINARIO" >&2
  exit 1
}

if ! docker image inspect "$IMAGEM" >/dev/null 2>&1; then
  echo "==> imagem $IMAGEM ausente; baixando com docker pull"
  docker pull "$IMAGEM"
fi

rm -rf "$TRABALHO"
mkdir -p "$TRABALHO/backups" "$TRABALHO/tests"
cp "$RAIZ/teste/tests/basics.yaml" "$TRABALHO/tests/basics.yaml"

export SENTINELA_CONFIG_DIR="$TRABALHO/config"
export SENTINELA_DATA_DIR="$TRABALHO/data"

limpar() {
  docker rm -f "$ORIGEM" >/dev/null 2>&1 || true
  if [ "$MANTER" -eq 0 ]; then
    "$BINARIO" cleanup >/dev/null 2>&1 || true
  else
    echo "==> sandbox mantido; remova depois com: $BINARIO cleanup"
  fi
}
trap limpar EXIT

echo "==> criando PostgreSQL de origem"
docker run -d --rm --name "$ORIGEM" \
  -e POSTGRES_PASSWORD=demo -e POSTGRES_DB=sentinela "$IMAGEM" >/dev/null

echo "==> aguardando PostgreSQL de origem"
pronto=0
for _ in $(seq 1 60); do
  if docker exec "$ORIGEM" psql -U postgres -d sentinela -tAc 'SELECT 1' >/dev/null 2>&1; then
    sleep 0.5
    if docker exec "$ORIGEM" psql -U postgres -d sentinela -tAc 'SELECT 1' >/dev/null 2>&1; then
      pronto=1
      break
    fi
  fi
  sleep 1
done
[ "$pronto" -eq 1 ] || {
  echo "erro: PostgreSQL de origem não ficou pronto" >&2
  exit 1
}

echo "==> criando dados e gerando dump"
docker exec "$ORIGEM" psql -U postgres -d sentinela -q \
  -c "CREATE TABLE users (id serial PRIMARY KEY, email text NOT NULL UNIQUE);" \
  -c "INSERT INTO users (email) VALUES ('alice@example.com'), ('bob@example.com');"
docker exec "$ORIGEM" pg_dump -U postgres -d sentinela -Fc > "$TRABALHO/backups/demo.dump"
docker rm -f "$ORIGEM" >/dev/null

if [ "$MODO" = "falha" ]; then
  echo "==> modo --falha: adicionando teste que deve falhar"
  cat > "$TRABALHO/tests/basics.yaml" <<'YAML'
tests:
  - name: users_exist
    sql: SELECT count(*) FROM users
    expect: { op: gte, value: 2 }

  - name: deve_falhar_de_proposito
    sql: SELECT count(*) FROM users
    expect: { op: gt, value: 999 }
YAML
fi

echo "==> configurando ambiente demo"
"$BINARIO" env add demo \
  --backup-dir "$TRABALHO/backups" \
  --tests-dir "$TRABALHO/tests" \
  --postgres-image "$IMAGEM"

echo "==> listando backups"
"$BINARIO" backups list --env demo

echo "==> executando prova de restauração"
set +e
"$BINARIO" run --env demo
EXIT_RUN=$?
set -e

echo "==> histórico"
"$BINARIO" history --env demo

RUN_ID="$("$BINARIO" history --env demo | awk 'NR==2{print $1}')"
EXIT_RELATORIO=0
if [ -n "${RUN_ID:-}" ]; then
  echo "==> verificando relatório"
  set +e
  "$BINARIO" report show "$RUN_ID"
  EXIT_RELATORIO=$?
  set -e
  echo
  echo "JSON:  $SENTINELA_DATA_DIR/reports/$RUN_ID.json"
  echo "HTML:  $SENTINELA_DATA_DIR/reports/$RUN_ID.html"
fi

if [ "$MODO" = "falha" ]; then
  ESPERADO=1
else
  ESPERADO=0
fi

if [ "$EXIT_RUN" -ne "$ESPERADO" ]; then
  echo "erro: 'run' saiu com $EXIT_RUN; esperado $ESPERADO" >&2
  exit 1
fi
if [ "$EXIT_RELATORIO" -ne 0 ]; then
  echo "erro: verificação do relatório falhou (exit $EXIT_RELATORIO)" >&2
  exit 1
fi

echo "OK: demo concluído (exit $EXIT_RUN)"
if [ "$MODO" = "ok" ]; then
  echo "Abra o HTML acima no navegador ou rode novamente com --falha para ver um teste falhando."
fi
