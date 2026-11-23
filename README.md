# Sentinela 0.1

Sentinela prova localmente que um backup PostgreSQL é restaurável. Ele sobe um
PostgreSQL descartável em Docker, restaura o dump, executa testes SQL e gera um
relatório em JSON/HTML com SHA-256 e assinatura Ed25519.

O fluxo inteiro roda na sua máquina: nenhum byte do backup ou dos dados sai do
host. O Sentinela também **não faz `docker pull`**: a imagem do PostgreSQL 16
viaja no pacote e é carregada com `docker load` na primeira execução.

```
backup (.dump)
      |
      v
pg_restore --list + pg_restore --no-owner
      |
      v
container PostgreSQL efêmero (rede internal, tmpfs, limites de CPU/memória)
      |
      v
testes SQL (transação sempre revertida)
      |
      v
relatório JSON + HTML + SHA-256 + assinatura Ed25519
```

## Usar sem instalar Rust

Quem só vai **usar** o Sentinela não precisa de Rust nem de compilador. Há dois
caminhos:

1. **Baixar um pacote pronto** na aba *Releases* do repositório:

   | Pacote | Sistema |
   | --- | --- |
   | `sentinela-<versao>-linux-x86_64.tar.gz` | Linux x86_64 |
   | `sentinela-<versao>-windows-x86_64.zip` | Windows x86_64 |

   Cada pacote traz `sentinela`, `sentinela-server`, `postgres16.tar.gz`
   (imagem do PostgreSQL 16), `README.md` e `COMO-USAR.txt`. O arquivo
   `.sha256` publicado ao lado serve para conferir a integridade do download.

2. **Gerar o pacote você mesmo**, em uma máquina que tenha Rust e Docker — uma
   vez só:

   ```bash
   ./scripts/preparar-imagem.sh       # gera postgres16.tar.gz (usa Docker)
   ./scripts/empacotar.sh            # Linux e macOS
   pwsh ./scripts/empacotar.ps1      # Windows
   ```

   No Windows, `scripts/preparar-imagem.ps1` gera `postgres16.tar` (sem
   compressão). O Sentinela aceita as duas extensões. O resultado fica em
   `dist/` e pode ser copiado para qualquer máquina.

Em ambos os casos a máquina de destino precisa apenas de **Docker em execução**.
Os executáveis são autossuficientes: o SQLite é compilado junto, os arquivos do
painel web estão embutidos no binário e a imagem do banco acompanha o pacote.

## Requisitos

**Para usar um pacote pronto:**

- Docker acessível pelo usuário atual. A imagem `postgres:16` **já vem no
  pacote** e é carregada automaticamente (`docker load`) na primeira execução —
  não é preciso `docker pull` nem internet. A imagem ocupa cerca de 700 MB no
  disco do Docker.

- Opcional: `pg_restore` no host. Quando presente, `sentinela backups list` e
  `sentinela run` fazem uma validação adicional com `pg_restore --list`; a
  validação autoritativa é sempre feita dentro do container.

**Para compilar a partir do código:**

- Rust stable (o repositório usa `rust-toolchain.toml`)
- Compilador C: `build-essential` no Linux, MSVC no Windows (o SQLite é
  compilado junto com o projeto)

## Build

```bash
cargo build --release
./target/release/sentinela --help
```

Para gerar o pacote distribuível (que dispensa Rust na máquina de destino):

```bash
./scripts/preparar-imagem.sh   # gera postgres16.tar.gz a partir do Docker local
./scripts/empacotar.sh         # Linux e macOS
pwsh ./scripts/empacotar.ps1   # Windows (-ExigirImagem no CI)
```

`preparar-imagem.sh`/`.ps1` fazem `docker pull postgres:16` **uma única vez**, na
máquina de quem empacota. O Sentinela em si nunca acessa a rede.

Durante o desenvolvimento:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Instalar o comando `sentinela`

`cargo build --release` gera `./target/release/sentinela`, mas o terminal só
reconhece `sentinela ...` se o executável estiver no `PATH`. Instale-o no
diretório de binários do Cargo:

```bash
cargo install --path crates/sentinela-cli
command -v sentinela   # precisa imprimir algo como /home/voce/.cargo/bin/sentinela
sentinela --version
```

O `cargo install` usa `$CARGO_HOME/bin` (por padrão `~/.cargo/bin`). Esse
diretório precisa estar no `PATH` do shell. Adicione uma das linhas abaixo ao
arquivo de inicialização correspondente e reabra o terminal:

```bash
# bash — ~/.bashrc
export PATH="$HOME/.cargo/bin:$PATH"
```

```bash
# zsh — ~/.zshrc
export PATH="$HOME/.cargo/bin:$PATH"
```

```fish
# fish — ~/.config/fish/config.fish
fish_add_path $HOME/.cargo/bin
```

Se você personalizou `CARGO_HOME`, use `"$CARGO_HOME/bin"` no lugar de
`"$HOME/.cargo/bin"`. Depois de reabrir o terminal, confirme com
`command -v sentinela` e `sentinela --help`. Em WSL, o `PATH` do Linux e o do
Windows são separados: instale e configure o binário dentro do WSL.

## Painel web local

O binário `sentinela-server` serve um painel navegável (Execuções, Relatório,
Ambientes e Limpeza) em `127.0.0.1`. Ele usa exatamente os mesmos diretórios,
configurações e relatórios assinados da CLI.

```bash
cargo build --release
./target/release/sentinela-server --port 8787
# o navegador abre sozinho em http://127.0.0.1:8787/
```

- **Duplo clique e usa:** ao iniciar, o servidor abre o painel sozinho — no app
  instalado, numa janela de aplicativo ou no navegador padrão. `--no-open`
  desativa esse comportamento.
- **Aplicativo instalado (PWA):** o painel é instalável no Edge/Chrome. Se você
  instalar o Sentinela como aplicativo, o servidor abre direto nele na próxima
  inicialização. Sem PWA instalado, o servidor abre o painel em uma janela de
  aplicativo do Chrome/Edge (`--app`), sem abas e sem barra de endereço; se não
  houver Chromium, abre no navegador padrão. Use `--browser` para forçar o
  navegador. Se o app ainda não estiver instalado, o painel mostra o botão
  **Instalar aplicativo** (o navegador exige esse clique; não há instalação
  silenciosa). Instale o app na mesma porta em que o servidor vai rodar
  (padrão 8787).
- **WSL:** rodando dentro do WSL, o servidor detecta o PWA instalado no Windows
  (via `/mnt/c`) e usa o Chrome/Edge do Windows para a janela de aplicativo.
  Sem navegador do Windows, cai no `xdg-open` do Linux.
- **Instância única:** se já houver um painel na porta, o programa não inicia um
  segundo servidor — apenas abre a página que já está no ar.
- **Encerrar:** o botão no canto inferior esquerdo do painel chama
  `POST /api/shutdown`, mostra "Servidor encerrado" e desliga o processo com
  desligamento gracioso (as requisições em andamento terminam antes).
- O servidor escuta **somente** em `127.0.0.1` e exige um token local gerado a
  cada inicialização, injetado no HTML. Nenhum dado sai da máquina.
- As telas usam os arquivos reais: ambientes em `envs/*.yaml`, histórico no
  SQLite, backups descobertos no diretório configurado e relatórios JSON/HTML
  assinados.
- "Executar prova de restauração" chama o mesmo fluxo da CLI (`run`) e o painel
  acompanha o resultado. Só uma execução roda por vez.
- A limpeza lista e remove recursos Docker com `label=sentinela=true`; ela é
  global e preserva histórico, relatórios e arquivos de backup.
- Remover um ambiente apaga apenas o YAML e o registro do ambiente; o histórico
  de execuções continua disponível.
- Os assets do painel ficam embutidos no binário em `crates/sentinela-server/web/`.

Requisitos adicionais: os mesmos da CLI — Docker para usar e Rust apenas para
compilar. A imagem do PostgreSQL 16 acompanha o pacote.

## Início rápido

<<<<<<< HEAD
1. Garanta a imagem do PostgreSQL 16 no Docker (uma vez só):
=======
Antes de executar os comandos abaixo, instale o binário e confirme que ele está
no `PATH` conforme [Instalar o comando `sentinela`](#instalar-o-comando-sentinela).

1. Baixe a imagem que será usada no sandbox:
>>>>>>> 4dbb773 (docs(cli): documentar cargo install e PATH do ~/.cargo/bin)

   ```bash
   ./scripts/preparar-imagem.sh   # gera postgres16.tar.gz e carrega no Docker
   ```

   Alternativa: `docker pull postgres:16`.

2. Crie um ambiente apontando para o diretório de dumps:

   ```bash
   sentinela env add production \
     --backup-dir /var/backups/postgres \
     --tests-dir ./tests/production \
     --postgres-version 16
   ```

3. Confira os backups descobertos (nome, tamanho, SHA-256 e validação):

   ```bash
   sentinela backups list --env production
   ```

4. Execute a prova de restauração:

   ```bash
   sentinela run --env production
   ```

   Por padrão o backup mais recente é usado. Para escolher um arquivo
   específico, informe o nome ou um prefixo único:

   ```bash
   sentinela run --env production --backup 2026-11-11
   ```

5. Veja o histórico e verifique o relatório assinado:

   ```bash
   sentinela history --env production
   sentinela report show <run_id>
   sentinela report show <run_id> --json
   ```

6. Remova recursos órfãos quando necessário:

   ```bash
   sentinela cleanup
   ```

## Comandos

| Comando | O que faz |
| --- | --- |
| `env add <nome> --backup-dir <dir>` | Cria/atualiza um ambiente em `envs/<nome>.yaml`. |
| `env list` | Lista os ambientes configurados. |
| `env remove <nome>` | Remove a configuração do ambiente; o histórico é preservado. |
| `backups list --env <nome>` | Lista dumps com tamanho, mtime, SHA-256 e validação `pg_restore --list` no host. |
| `run --env <nome> [--backup <id\|prefixo>] [--keep]` | Restaura o backup e roda os testes SQL. |
| `history [--env <nome>] [--status <status>]` | Mostra o histórico local. |
| `report show <run_id> [--json]` | Recalcula o SHA-256, verifica a assinatura e mostra o resumo. |
| `cleanup` | Remove containers, redes e volumes com `label=sentinela=true`. |

`--status` aceita `ok`, `failed` (qualquer status diferente de `ok`),
`tests_failed`, `restore_failed` e `infra_error`.

### `run --keep`

`--keep` mantém o container e a rede da execução para depuração, e também pula
a limpeza de órfãos no início. Use `sentinela cleanup` quando terminar.

## Códigos de saída

| Código | Significado |
| --- | --- |
| `0` | Restore concluído e todos os testes passaram. |
| `1` | Restore concluído, mas ao menos um teste SQL falhou. |
| `2` | O backup não pôde ser validado/restaurado. |
| `3` | Erro de uso, configuração, Docker, persistência, relatório ou interrupção. |

`--help` e `--version` saem com `0`.

## Diretórios

Por padrão:

| Caminho | Conteúdo |
| --- | --- |
| `~/.config/sentinela/config.yaml` | Defaults globais de `env add`. |
| `~/.config/sentinela/envs/<nome>.yaml` | Configuração de cada ambiente. |
| `~/.local/share/sentinela/sentinela.db` | Histórico SQLite. |
| `~/.local/share/sentinela/reports/` | Relatórios `<run_id>.json`, `.html`, `.sha256`, `.sig`, `.pub`. |
| `~/.local/share/sentinela/keys/` | Chave privada Ed25519 (`0600`) e chave pública. |

Overrides úteis, especialmente em testes:

```bash
SENTINELA_CONFIG_DIR=/tmp/sentinela/config \
SENTINELA_DATA_DIR=/tmp/sentinela/data \
sentinela env list
```

Também são respeitados `XDG_CONFIG_HOME` e `XDG_DATA_HOME`. No Windows, quando
`HOME` não está definido, o `USERPROFILE` é usado como diretório base.

## Configuração de ambiente

Exemplo completo em [`examples/envs/production.yaml`](examples/envs/production.yaml).
Caminhos relativos são resolvidos a partir do diretório do arquivo YAML.

```yaml
name: production

backup:
  provider: logical      # único provider implementado no 0.1
  directory: /var/backups/postgres
  glob: "*.dump"         # padrão simples, relativo a directory

postgres:
  version: "16"
  image: postgres        # se não tiver tag, recebe :<version>
  user: postgres
  database: sentinela

docker:
  memory_mb: 512
  cpus: 1.0
  readiness_timeout_secs: 60
  restore_timeout_secs: 600
  data_tmpfs_mb: 1024

tests:
  directory: ./tests/production
  statement_timeout_ms: 5000
```

O nome do backup é o nome do arquivo (por exemplo, `2026-11-11.dump`).

> **PgBackRest:** `provider: pgbackrest` existe apenas como stub documentado no
> 0.1 e falha na descoberta com erro de configuração, sem subir container.

## Formato dos testes SQL

Arquivos `*.yaml`/`*.yml` no diretório de testes. Cada arquivo pode conter uma
lista direta ou uma chave `tests:`.

```yaml
tests:
  - name: users_exist
    sql: SELECT count(*) FROM users
    expect: { op: gte, value: 1 }

  - name: users_have_email
    sql: SELECT bool_and(email IS NOT NULL) FROM users
    expect: { op: is_true }

  - name: recent_orders
    sql: |
      SELECT count(*) FROM orders
      WHERE created_at > now() - interval '30 days'
    expect: { op: gt, value: 0 }
    timeout_ms: 2000
```

Regras:

- O SQL deve retornar **um único escalar**: booleano ou numérico. Consultas que
  retornam texto/linhas falham como erro de teste.
- Cada teste roda em uma transação própria, sempre revertida — mesmo que o SQL
  escreva dados.
- `timeout_ms` sobrescreve `tests.statement_timeout_ms` para aquele teste.
- Nomes de teste são únicos em todo o diretório.

Operadores de `expect`:

| `op` | `value` | Semântica |
| --- | --- | --- |
| `eq`, `ne` | número ou booleano | igualdade/desigualdade |
| `gt`, `gte`, `lt`, `lte` | número | comparação numérica |
| `is_true`, `is_false` | — | valor booleano |
| `not_null` | — | valor não nulo |

O relatório grava apenas nome, status, valor escalar e duração. Mensagens de
falha não incluem o valor observado nem linhas do banco.

## Isolamento do sandbox

Para cada `run`, o Sentinela:

- cria a rede `sentinela-net-<run_id>` com `internal: true`;
- cria o container `sentinela-run-<run_id>` com labels `sentinela=true`;
- monta o backup como somente leitura em `/backup/input.dump`;
- usa tmpfs para `/var/lib/postgresql/data`;
- aplica limites de memória e CPU;
- gera uma senha aleatória que nunca aparece em logs ou relatórios;
- deixa o host alcançar o IP do container na rede interna (sem publicar portas)
  para executar os testes via `tokio-postgres`;
- aguarda o banco aceitar consultas (`SELECT 1`) de forma estável antes de
  restaurar;
- remove container e rede em todos os caminhos (sucesso, falha, erro e
  SIGINT/SIGTERM), exceto com `--keep`.

No início de cada execução (sem `--keep`), recursos órfãos com
`label=sentinela=true` são removidos.

## Relatórios e integridade

Cada run grava, em `reports/`:

| Arquivo | Conteúdo |
| --- | --- |
| `<run_id>.json` | Relatório canônico. |
| `<run_id>.html` | Versão visual autocontida (CSS inline, sem JS/CDN). |
| `<run_id>.sha256` | SHA-256 do JSON. |
| `<run_id>.sig` | Assinatura Ed25519 em base64. |
| `<run_id>.pub` | Chave pública Ed25519 em base64. |

A assinatura é Ed25519 sobre o **digest SHA-256 cru** (32 bytes) do JSON. A
chave privada fica em `keys/report-signing.key` com permissão `0600`.

`sentinela report show <run_id>` recalcula o hash, compara com o sidecar e
verifica a assinatura antes de exibir qualquer resumo. O `--json` só imprime o
JSON bruto depois da verificação. Se o JSON for alterado, o comando falha com
código `3`.

Antes de montar o dump no container, o `run` recalcula o SHA-256 do arquivo
escolhido e compara com o hash calculado na descoberta. Se existir um checksum
ao lado do dump (`<dump>.sha256` ou `<dump sem extensão>.sha256`), ele também
precisa concordar. Divergência aborta a execução antes de qualquer restore:
o relatório nunca atesta um hash diferente do arquivo efetivamente restaurado.

O relatório inclui tempos separados para descoberta/SHA-256, subida/readiness,
validação/restore, testes e total.

Diagnósticos de `stderr` do `pg_restore` são exibidos apenas no terminal e não
entram no JSON, no HTML nem no SQLite. Isso evita persistir acidentalmente
valores de linhas do cliente em mensagens de erro do PostgreSQL.

## Teste ponta a ponta

O teste E2E é ignorado por padrão porque exige Docker e a imagem local:

```bash
docker pull postgres:16
cargo test -p sentinela-cli --test docker_e2e -- --ignored --nocapture
```

Ele inicia um PostgreSQL de origem, cria uma tabela, gera um dump com
`pg_dump -Fc`, executa `env add`, `backups list`, `run`, `history` e
`report show`, confere os cinco artefatos e valida que adulterar o JSON quebra a
verificação de integridade.

## Primeira execução manual, passo a passo

Se você quer validar o fluxo sem o teste automatizado:

```bash
# 1. Build e instalação no PATH
cargo build --release
cargo install --path crates/sentinela-cli
command -v sentinela

# 2. Imagem local (o Sentinela não faz pull; usa postgres16.tar[.gz] se existir)
./scripts/preparar-imagem.sh

# 3. Diretórios de trabalho isolados
export SENTINELA_CONFIG_DIR=/tmp/sentinela-demo/config
export SENTINELA_DATA_DIR=/tmp/sentinela-demo/data
mkdir -p /tmp/sentinela-demo/backups /tmp/sentinela-demo/tests

# 4. Um dump de exemplo (a partir de um PostgreSQL de origem)
docker run -d --rm --name sentinela-demo-source \
  -e POSTGRES_PASSWORD=demo -e POSTGRES_DB=sentinela postgres:16
until docker exec sentinela-demo-source pg_isready -U postgres -d sentinela; do sleep 1; done
docker exec sentinela-demo-source psql -U postgres -d sentinela \
  -c "CREATE TABLE users (id serial PRIMARY KEY, email text NOT NULL UNIQUE);" \
  -c "INSERT INTO users (email) VALUES ('alice@example.com'), ('bob@example.com');"
docker exec sentinela-demo-source pg_dump -U postgres -d sentinela -Fc \
  > /tmp/sentinela-demo/backups/demo.dump
docker rm -f sentinela-demo-source

# 5. Testes
cat > /tmp/sentinela-demo/tests/basics.yaml <<'YAML'
tests:
  - name: users_exist
    sql: SELECT count(*) FROM users
    expect: { op: gte, value: 2 }
  - name: emails_are_unique
    sql: SELECT count(DISTINCT email) = count(*) FROM users
    expect: { op: is_true }
YAML

# 6. Configuração e execução
sentinela env add demo \
  --backup-dir /tmp/sentinela-demo/backups \
  --tests-dir /tmp/sentinela-demo/tests \
  --postgres-image postgres:16

sentinela backups list --env demo
sentinela run --env demo

# 7. Verificação do relatório
sentinela history --env demo
sentinela report show <run_id>
```

## Limitações do 0.1

- Apenas dumps lógicos (`pg_dump -Fc`); PgBackRest é um stub.
- A imagem do PostgreSQL precisa existir localmente ou o arquivo
  `postgres16.tar[.gz]` precisa estar ao lado do executável (é o formato do
  pacote distribuído).
- Os testes conectam ao IP do container na rede `internal`; nenhuma porta é
  publicada e o container continua sem saída para a internet.
- O bind mount do backup não aceita caminhos com `:` no 0.1.
- As ferramentas `pg_dump`/`pg_restore` usadas para gerar o dump de exemplo são
  as da imagem oficial do PostgreSQL; o Sentinela em si só exige
  `pg_restore --list` opcional no host.

## Estrutura do workspace

```
crates/
  sentinela-core/     configuração, modelos, expectativas, erros, hash e paths
  sentinela-backup/   descoberta/validação de backups (logical + stub pgBackRest)
  sentinela-runner/   Docker, sandbox, restore e testes SQL
  sentinela-store/    histórico SQLite (runs, testes, relatórios)
  sentinela-cli/      comandos, relatórios JSON/HTML e assinatura Ed25519
  sentinela-server/   painel web local (axum) e assets embutidos do painel
examples/             config, ambiente e testes de exemplo
```
