# Tarefa 3.0: Empacotamento Docker — HAProxy + Dockerfile multi-stage + `docker-compose.yml`

<critical>Ler os arquivos de prd.md e techspec.md desta pasta, se você não ler esses arquivos sua tarefa será invalidada</critical>

## Visão Geral

Empacotar a solução para execução pela engine da Rinha: configuração HAProxy em round-robin puro, `Dockerfile` multi-stage que compila os binários, gera os artefatos do dataset e produz uma imagem runtime mínima (`distroless/cc-debian12`), e `docker-compose.yml` declarando o load balancer (`lb`) mais duas instâncias da API (`api-1`, `api-2`) com limites de CPU/memória que somam ≤ 1.0 CPU / ≤ 350 MB. Esta tarefa entrega o sistema executável ponta a ponta via `docker compose up`.

<skills>
### Conformidade com Skills Padrões

- **`devops-engineer`** — Dockerfile multi-stage, configuração de compose e limites de recursos.
- **`verification-before-completion`** — confirmar `docker buildx build`, `docker compose up`, `/ready=200` em ambas instâncias e p99 ≤ 1 ms via `oha`.
- **`no-workarounds`** — limites de recursos respeitados pelo design, não por tuning artificial.
</skills>

<requirements>
- `infra/haproxy/haproxy.cfg`: 1 frontend HTTP em `:9999`, 1 backend `balance roundrobin` apontando para `api-1:8080` e `api-2:8080`. Sem ACL, sem inspeção de payload, sem condicional, sem transformação de corpo.
- `Dockerfile` multi-stage com 3 estágios:
  - `builder`: compila `build-dataset` e `api` em release com `target-cpu=x86-64-v3`.
  - `dataset`: executa `build-dataset` para gerar `references.i8.bin` + `labels.bits`.
  - `runtime`: `gcr.io/distroless/cc-debian12` recebendo apenas o binário `api` + artefatos.
- Todos os `FROM` fixam `--platform=linux/amd64` (build cross-arch obrigatório em Apple Silicon via `docker buildx`).
- `docker-compose.yml` com:
  - Serviço `lb` (HAProxy `haproxytech/haproxy-alpine:2.9`): `cpus: 0.10`, `memory: 30M`, expõe porta `9999:9999`.
  - Serviço `api-1` e `api-2`: `cpus: 0.45`, `memory: 160M` cada.
  - Volume read-only `dataset-vol` populado a partir do estágio `dataset` do Dockerfile, montado em ambas APIs.
  - Rede com driver `bridge` (modos `host` e `privileged` PROIBIDOS).
- Soma de `deploy.resources.limits`: ≤ 1.0 CPU e ≤ 350 MB.
- Imagens externas referenciadas devem ser públicas, `linux-amd64` compatíveis.
- `/ready` deve responder `200` em ambas instâncias após `docker compose up`.
- Round-robin verificável: requisições alternadas entre `api-1` e `api-2`.
</requirements>

## Subtarefas

- [ ] 3.1 Escrever `infra/haproxy/haproxy.cfg` com frontend `:9999` e backend `balance roundrobin` para `api-1:8080`/`api-2:8080`.
- [ ] 3.2 Escrever `Dockerfile` multi-stage com estágios `builder`, `dataset` e `runtime` (distroless/cc-debian12, `--platform=linux/amd64`).
- [ ] 3.3 Validar build cross-arch: `docker buildx build --platform linux/amd64 .` em Apple Silicon.
- [ ] 3.4 Escrever `docker-compose.yml` declarando `lb`, `api-1`, `api-2`, volume `dataset-vol` e rede `bridge` com limites de recursos especificados.
- [ ] 3.5 Verificar footprint: `docker stats` confirmando soma ≤ 1.0 CPU / ≤ 350 MB sob carga.
- [ ] 3.6 Smoke test: `docker compose up`, polling em `/ready` até `200`, `oha -n 5000 -c 50 http://localhost:9999/fraud-score` confirmando p99 ≤ 1 ms e zero erros.
- [ ] 3.7 Verificar round-robin: log/trace mostrando distribuição alternada entre `api-1` e `api-2`.

## Detalhes de Implementação

Ver seções "Visão Geral dos Componentes", "Pontos de Integração", "Considerações Técnicas" e "Riscos Conhecidos" da [techspec.md](./techspec.md). Em particular: o estágio `dataset` do Dockerfile é executado em build-time e popula o volume read-only consumido pelos serviços `api-*` em runtime; o split de recursos `0.10/30 + 0.45/160 + 0.45/160` foi calibrado para deixar folga ao HAProxy e cobrir mmap (~48 MB) + heap + page cache compartilhado pelas APIs.

## Critérios de Sucesso

- `docker compose up` sobe `lb`, `api-1` e `api-2` sem erros.
- `curl http://localhost:9999/ready` retorna `200` após warmup completo das duas instâncias.
- `oha -n 5000 -c 50 http://localhost:9999/fraud-score` reporta p99 ≤ 1 ms e zero `5xx`.
- `docker stats` confirma soma de CPU ≤ 1.0 e memória ≤ 350 MB sob carga.
- Round-robin verificado por inspeção (logs, métricas internas ou contagem de requisições por instância).
- `docker buildx build --platform linux/amd64 .` funciona em Apple Silicon.

## Testes da Tarefa

- [ ] Testes de unidade — não se aplica (configuração de infra).
- [ ] Testes de integração — `docker compose up` + healthcheck em `/ready` em ambas instâncias; `oha` confirmando p99 ≤ 1 ms; verificação de footprint via `docker stats`; validação de round-robin.
- [ ] Testes E2E — smoke completo: subir compose, executar carga incremental simulando o k6 da Rinha (ou usar o script oficial em `test/`), confirmar zero `5xx`.

<critical>SEMPRE CRIE E EXECUTE OS TESTES DA TAREFA ANTES DE CONSIDERÁ-LA FINALIZADA</critical>

## Arquivos relevantes

- `infra/haproxy/haproxy.cfg`
- `Dockerfile` (multi-stage)
- `docker-compose.yml`
- `.dockerignore`
- `crates/api/target/release/api` (consumido)
- `crates/build-dataset/target/release/build-dataset` (consumido)
- `target/dataset/references.i8.bin`, `target/dataset/labels.bits` (consumidos)
