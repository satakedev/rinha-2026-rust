# Tarefa 4.0: Pacote de submissão — `info.json`, branch `submission` e README

<critical>Ler os arquivos de prd.md e techspec.md desta pasta, se você não ler esses arquivos sua tarefa será invalidada</critical>

## Visão Geral

Empacotar a entrega oficial conforme SUBMISSAO.md: criar `info.json` com os metadados do participante, manter a branch `main` com todo o código-fonte e criar a branch `submission` contendo apenas os artefatos exigidos para o teste oficial (`docker-compose.yml` + `info.json`). Adicionar `README.md` na `main` com diagrama de topologia, instruções de build/run e nota sobre `docker buildx --platform linux/amd64` para mantenedores em Apple Silicon. Esta tarefa é o gate final antes de abrir a issue `rinha/test`.

<skills>
### Conformidade com Skills Padrões

- **`crafting-effective-readmes`** — README claro com diagrama de topologia, instruções de build/run e contexto da estratégia.
- **`verification-before-completion`** — confirmar estrutura de branches, validade do `info.json` e dry-run da issue `rinha/test` antes de declarar pronto.
- **`writing-clearly-and-concisely`** — README e mensagens em prosa direta.
</skills>

<requirements>
- `info.json` contendo todos os campos obrigatórios: `participants`, `social`, `source-code-repo`, `stack`, `open_to_work`.
- Branch `main` mantém o código-fonte completo (todos os crates, Dockerfile, compose, infra, tasks, recursos).
- Branch `submission` contém APENAS:
  - `docker-compose.yml` na raiz.
  - `info.json` na raiz.
  - Sem código-fonte, sem Dockerfile, sem `crates/`, sem `infra/haproxy/haproxy.cfg`.
- `README.md` na `main` com:
  - Diagrama de topologia (HAProxy → api-1/api-2 + volume compartilhado).
  - Instruções de build local (`cargo build --release`) e execução (`docker compose up`).
  - Nota explícita sobre `docker buildx build --platform linux/amd64` para Apple Silicon.
  - Resumo da estratégia (k-NN exato com SIMD AVX2, quantização `i8`, single-thread per instance).
  - Estrutura de diretórios autoexplicativa.
- Issue `rinha/test` aberta no repositório retorna resposta automática válida da engine em até 5 tentativas de prévia.
- Todas as URLs e identificadores no `info.json` apontam para o repositório real do participante.
</requirements>

## Subtarefas

- [ ] 4.1 Escrever `info.json` com `participants`, `social`, `source-code-repo`, `stack`, `open_to_work`.
- [ ] 4.2 Escrever `README.md` na `main` com diagrama de topologia, instruções de build/run e nota sobre `buildx`.
- [ ] 4.3 Criar branch `submission` contendo apenas `docker-compose.yml` + `info.json`; verificar ausência de código-fonte.
- [ ] 4.4 Validar localmente que `docker compose up` na branch `submission` sobe a stack completa (puxando imagens públicas).
- [ ] 4.5 Abrir issue de teste `rinha/test` (dry-run) e validar que a engine retorna resultado válido em até 5 tentativas.

## Detalhes de Implementação

Ver SUBMISSAO.md (referenciado pela [techspec.md](./techspec.md), seção "Funcionalidades Principais — F5") para o formato exato do `info.json` e a estrutura esperada das duas branches. A `submission` deve ser orphan ou cuidadosamente curada para não vazar código-fonte; a forma mais segura é manter um branch isolado que contém apenas os dois arquivos exigidos. As imagens referenciadas no `docker-compose.yml` devem ser públicas e `linux-amd64`.

## Critérios de Sucesso

- `info.json` valida contra o schema descrito em SUBMISSAO.md (todos os campos presentes e bem formados).
- `git ls-tree -r submission` mostra exatamente `docker-compose.yml` e `info.json` (e nada mais).
- `docker compose up` na branch `submission` sobe a stack completa puxando apenas imagens públicas, sem necessidade de código-fonte local.
- `README.md` da `main` é autoexplicativo: um leitor novo consegue clonar, buildar, executar e entender a estratégia.
- Issue `rinha/test` aberta no repositório recebe resposta automática válida da engine.

## Testes da Tarefa

- [ ] Testes de unidade — não se aplica (artefatos de submissão).
- [ ] Testes de integração — `git checkout submission && docker compose up` em ambiente limpo (sem código-fonte) confirma que a stack sobe e responde a `/ready` e `/fraud-score`.
- [ ] Testes E2E — abrir issue `rinha/test` (dry-run) e validar resposta automática da engine; conferir resultado oficial dentro do limite de tentativas de prévia.

<critical>SEMPRE CRIE E EXECUTE OS TESTES DA TAREFA ANTES DE CONSIDERÁ-LA FINALIZADA</critical>

## Arquivos relevantes

- `info.json` (raiz, em ambas branches)
- `docker-compose.yml` (raiz, em ambas branches)
- `README.md` (apenas na `main`)
- Branches: `main` (código-fonte completo), `submission` (apenas artefatos da entrega)
