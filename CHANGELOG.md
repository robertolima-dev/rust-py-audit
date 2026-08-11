# Changelog

Todas as mudanças relevantes deste projeto são registradas aqui.

O formato segue [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/),
e o versionamento segue [SemVer](https://semver.org/lang/pt-BR/).

## [0.3.3] — não publicada

Correção do passo de verificação pós-upload introduzido na 0.3.2.

### Corrigido

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
