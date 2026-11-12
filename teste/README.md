# Teste rápido do Sentinela

Esta pasta contém um demo automatizado para provar que um backup PostgreSQL é
restaurável. O script usa Docker, gera um dump de exemplo e roda o fluxo
completo do Sentinela: descoberta, restore, testes SQL, histórico e relatório
assinado.

Tudo roda localmente: nenhum dado sai da sua máquina.

## Requisitos

- Docker acessível pelo seu usuário (`docker run hello-world`)
- Rust stable (`rustup`) e um compilador C (`gcc`/`clang`) para compilar, caso
  o binário ainda não exista
- Cerca de 500 MB de espaço livre para a imagem `postgres:16`

O script baixa a imagem `postgres:16` automaticamente se ela não existir. O
Sentinela em si nunca faz `docker pull`; isso é feito apenas pelo script de
teste.

## Rodar o demo

Na raiz do repositório:

```bash
./teste/rodar-demo.sh
```

Resultado esperado: `run` termina com exit `0`, os 2 testes passam e o relatório
é verificado. O script imprime os caminhos do JSON e do HTML — abra o HTML no
navegador.

Para ver o Sentinela detectando uma falha de teste:

```bash
./teste/rodar-demo.sh --falha
```

Agora o `run` termina com exit `1` e o relatório mostra o teste
`deve_falhar_de_proposito` como falho. O script considera isso sucesso e sai com
`0`.

Para manter o sandbox após a execução (depuração):

```bash
./teste/rodar-demo.sh --manter
docker ps --filter label=sentinela=true
./target/release/sentinela cleanup
```

## Onde ficam os arquivos

O script usa um diretório isolado, `teste/.work/`, para não mexer na sua
configuração real do Sentinela:

```
teste/.work/
  backups/demo.dump
  tests/basics.yaml
  config/envs/demo.yaml
  data/sentinela.db
  data/reports/<run_id>.json|.html|.sha256|.sig|.pub
```

Remova com `rm -rf teste/.work`.

## Testar a integridade do relatório

Depois de rodar o demo:

```bash
export SENTINELA_CONFIG_DIR="$PWD/teste/.work/config"
export SENTINELA_DATA_DIR="$PWD/teste/.work/data"
BIN="$PWD/target/release/sentinela"

RUN=$("$BIN" history --env demo | awk 'NR==2{print $1}')
"$BIN" report show "$RUN"          # deve passar

cp "teste/.work/data/reports/$RUN.json" /tmp/relatorio-original.json
python3 - "teste/.work/data/reports/$RUN.json" <<'PY'
import json, sys
caminho = sys.argv[1]
dados = json.load(open(caminho))
dados["status"] = "failed"
open(caminho, "w").write(json.dumps(dados))
PY

"$BIN" report show "$RUN"; echo "exit=$?"   # deve sair com 3
cp /tmp/relatorio-original.json "teste/.work/data/reports/$RUN.json"
```

## Rodar o teste E2E oficial

O repositório também tem um teste ponta a ponta ignorado por padrão:

```bash
docker pull postgres:16
cargo test -p sentinela-cli --test docker_e2e -- --ignored --nocapture
```

## Problemas comuns

- **`permission denied` no Docker:** adicione seu usuário ao grupo `docker` e
  abra uma nova sessão (`sudo usermod -aG docker "$USER"`).
- **Falha ao compilar `libsqlite3-sys`:** instale um compilador C
  (`sudo apt install build-essential` ou `xcode-select --install`).
- **Porta/container em uso:** rode `./target/release/sentinela cleanup` e tente
  novamente.
- **Imagem ausente:** `docker pull postgres:16` (ou use
  `SENTINELA_TESTE_IMAGE=postgres:15 ./teste/rodar-demo.sh` com uma imagem já
  local).
