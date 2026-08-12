# Changelog

Todas as mudanças relevantes deste projeto são registradas aqui.

O formato segue [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/),
e o versionamento segue [SemVer](https://semver.org/lang/pt-BR/).

## [0.3.3] — não publicada

Correção do passo de verificação pós-upload introduzido na 0.3.2, mais o
trabalho de licença e lint de 12/08/2026 (**P1-4** e **P1-6**).

### Alterado

- **Publicação migrada para trusted publishing (OIDC).** O `release.yml` não
  usa mais `secrets.PYPI_API_TOKEN`: o GitHub emite um JWT curto afirmando
  repositório, workflow e ref, e o PyPI o troca por um token de upload válido
  por poucos minutos e restrito a este projeto. Acaba o segredo de longa
  duração que, se vazasse, publicaria até ser revogado à mão.

  O passo de upload deixou de ser `maturin upload` dentro de um
  `nick-fields/retry` e passou a ser dois `pypa/gh-action-pypi-publish`
  explícitos (`continue-on-error` na primeira, `if: outcome == 'failure'` na
  segunda) — a action oficial é a que faz a troca OIDC, e não dá para acioná-la
  por linha de comando de dentro do wrapper de retry. `skip-existing: true`
  mantém a segunda tentativa idempotente. É a mesma forma que `rust_py_cache` e
  `rust_py_rate_limit` já rodam.

  ⚠️ **Depende de configuração no PyPI**, e os valores têm que bater exatamente:
  `robertolima-dev/rust-py-audit`, workflow `release.yml`, environment `pypi`.
  O modo de falha é seguro — sem o publisher cadastrado, o job falha na troca
  do token, depois de construir tudo e antes de qualquer arquivo sair do
  runner. Nada é publicado pela metade.

  💡 A partir daqui a release passa a gerar **atestados PEP 740**, que o
  `maturin upload` não gerava. A verificação pós-upload já estava preparada:
  ela só conta `*.whl` e `*.tar.gz` desde a 0.3.2, recorte que nasceu do alarme
  falso da `rate_limit` 0.2.1. Sem ele, este primeiro release por OIDC seria
  reprovado por um motivo inventado.

- **Ensaio no TestPyPI por `workflow_dispatch`.** Job `publish-testpypi` novo,
  na forma que o `rust_py_rate_limit` já tem: um disparo manual constrói a
  matriz inteira e publica no índice de teste, para validar a troca de token
  num lugar onde nada é irreversível.

  Para isso, os cinco jobs de build passaram de
  `if: startsWith(github.ref, 'refs/tags/')` para
  `... || github.event_name == 'workflow_dispatch'` — sem essa mudança um
  disparo manual não construiria wheel nenhuma e o ensaio ficaria sem o que
  publicar.

  ⚠️ **O job `release` deliberadamente NÃO ganhou a cláusula de dispatch.** A
  assimetria é a proteção: disparo manual vai ao TestPyPI, só a tag chega ao
  PyPI de produção.

### Adicionado

- **Job `lint`** (`cargo fmt --check` + `cargo clippy -D warnings`) no
  `release.yml`, com `needs: [test, lint]` em todos os jobs de build e no de
  publicação — o lint barra a release, igual ao `test`. A toolchain é fixa
  (`dtolnay/rust-toolchain@1.88.0`) e não `stable`: com `-D warnings`, um
  clippy mais novo no runner reprovaria código que ninguém tocou.
- **`clippy.toml` e `Makefile`** compartilhados com os outros quatro
  `rust_py_*`. O `clippy.toml` sobe `too-many-arguments-threshold` para 10:
  os construtores PyO3 espelham a assinatura Python, e reagrupar em struct
  para agradar o lint pioraria a API que o usuário vê.

### Corrigido

- **Metadado de licença ausente no PyPI.** A 0.3.2 está publicada com
  `license_expression: null` e `license: ""` — ou seja, sem metadado de
  licença nenhum, só o classifier. A causa raiz não era a forma no
  `pyproject.toml` (que já usava `license = "MIT"`), e sim a **maturin velha**:
  versões anteriores à 1.14 não emitem `License-Expression`. O piso do
  `[build-system]` subiu de `>=1.5` para `>=1.14`, entrou
  `license-files = ["LICENSE"]` e o classifier `License :: OSI Approved` saiu,
  por ser mutuamente exclusivo com `License-Expression` no Metadata 2.4.
  Verificado: o `dist-info` gerado agora traz `License-Expression: MIT` e o
  texto da licença em `licenses/LICENSE`.

- **Formatação.** `cargo fmt --all` aplicado — eram 38 blocos divergentes, de
  longe o maior dos cinco. Mudança puramente de formatação, sem efeito em
  comportamento; os 50 testes Rust seguem passando.

### Conhecido

- `src/audit_logger.rs` ganhou um `#![allow(clippy::useless_conversion)]` de
  módulo. **Não é código a corrigir:** o lint aponta o *tipo de retorno* de
  `log`, `verify` e `flush_pending`, mas a conversão está no código que
  `#[pymethods]` gera — a macro do pyo3 0.22. `#[allow]` no bloco `impl` e na
  função foram testados e não silenciam, porque o código gerado fica fora do
  `impl`. `cache` (0.24), `monitor` (0.25) e `scheduler` (0.29) não emitem o
  aviso, o que confirma ser artefato da versão. O `allow` sai com o **P3-2**.

### Corrigido — pipeline (0.3.2)

- **Falso negativo na verificação pós-publicação.** O passo comparava
  `glob("dist/*")` com o que estava no PyPI. Nada quebrou aqui — o
  `maturin upload` não deixa arquivo extra em `dist/` —, mas o glob amplo é
  frágil: qualquer arquivo novo no diretório reprovaria uma release que deu
  certo, que foi exatamente o que aconteceu no `rust_py_rate_limit`. O passo
  agora só considera `*.whl` e `*.tar.gz`, os únicos tipos de arquivo que o
  PyPI hospeda.

- **Os testes passam a rodar em push e PR.** O `release.yml` é o único workflow
  destes repositórios e só disparava em tag, então o job `test` só rodava na
  hora do release — protegia a publicação, mas só avisava do problema no pior
  momento. Agora o workflow também dispara em `push` para `main` e em
  `pull_request`, e os jobs de build e o de publicação ganharam
  `if: startsWith(github.ref, 'refs/tags/')`. Essa guarda corrige um risco que
  existia antes: o job de release **não tinha `if:`**, então um
  `workflow_dispatch` manual a partir de um branch teria publicado no PyPI.

**Não vai ao PyPI sozinha:** a 0.3.2 está lá completa e esta versão só
mexe no pipeline. O número fica reservado e sai com a próxima mudança que
valha uma tag.

## [0.3.2] - 2026-08-11

Release de infraestrutura: nada muda no comportamento da biblioteca.

### Corrigido

- **A publicação não roda mais sem teste.** O `release.yml` era o único
  workflow do repositório e só tinha jobs de build: `git push --tags`
  publicava no PyPI sem nenhum `cargo test` nem `pytest`. Agora existe um job
  `test` e todos os jobs de wheel, o de sdist e o de release declaram
  `needs: test`. (BACKLOG2 P1-1)
- **O upload aguenta o 500 do PyPI.** O upload é arquivo por arquivo e sem
  transação — uma falha no meio deixa a versão publicada pela metade, e o PyPI
  nunca permite reusar um nome de arquivo. O passo de publicação agora tem
  retry (4 tentativas), continua usando `--skip-existing` para ser idempotente,
  confere o conjunto de artefatos **antes** de subir qualquer coisa (aborta se
  faltar o sdist) e valida contra a API do PyPI **depois**, listando nome por
  nome o que não chegou. (BACKLOG2 P1-9)

### Adicionado

- Este `CHANGELOG.md`. (BACKLOG2 P1-5)

## [0.3.1] e anteriores

Sem changelog retroativo. O histórico do que entrou em cada versão está no
`ROADMAP.md`, e o das mudanças no `git log`.

Resumo do que o projeto entrega hoje:

- `AuditLogger` com log append-only em JSONL e cadeia de hashes encadeada.
- `verify()` para detectar adulteração da cadeia.
- Integração com o ImmutableLog nos modos remoto e híbrido.
- Middlewares para FastAPI e Django.
- Wheels `abi3-py310` — uma wheel por plataforma, válida da 3.10 em diante.

### Limitação conhecida (não corrigida nesta versão)

⚠️ **Múltiplos processos escrevendo no mesmo arquivo quebram a cadeia.** Cada
processo mantém seu próprio `last_hash` em memória e não há `flock` no append,
então dois workers gunicorn apontando para o mesmo JSONL produzem uma cadeia
inconsistente — e `verify()` acusa corrupção. O modo de falha é silencioso até
alguém rodar `verify()`. Ver **P2-4** no `BACKLOG2.md`.
