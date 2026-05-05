# Tech Spec — Submissão Rinha de Backend 2026 (Detecção de Fraude por Busca Vetorial em Rust)

## Resumo Executivo

Vamos construir um serviço HTTP em Rust que vetoriza cada transação em 14 dimensões `i8` e executa um k-NN exato (k=5, distância euclidiana ao quadrado) por scan linear vetorizado com AVX2 sobre 3M de vetores `i8` mapeados em memória via `memmap2`. Duas instâncias da API (`api-1`, `api-2`), atrás de um HAProxy em round-robin puro, compartilham via volume read-only os artefatos pré-processados em build-time num Dockerfile multi-stage; o kernel compartilha as páginas do `mmap` entre as instâncias, mantendo a soma de RSS dentro de 350 MB.

A estratégia de execução é deliberadamente "single-thread por instância": Tokio `current_thread`, scan inline na própria task da request (sem `rayon`), heap top-5 em buffer de pilha e parser `serde_json` com structs zero-copy. Essa escolha evita jitter de work-stealing e mantém p99 determinístico — alinhado ao corte de 1 ms da fórmula de pontuação (AVALIACAO.md). Quantização `[0.0,1.0] → [0,100]` (sentinela `-1`) deixa erro de quantização << 0,5% de failure_rate e simplifica a aritmética AVX2 sem risco de overflow no acumulador `i32`.

## Arquitetura do Sistema

### Visão Geral dos Componentes

Componentes novos a serem criados:

- **`crates/shared`** *(library)* — vetorização, normalização, quantização e formato binário dos artefatos. Reutilizada pelo build offline e pelo runtime para garantir paridade bit-a-bit com o oráculo.
- **`crates/build-dataset`** *(binário)* — lê `references.json.gz`, `mcc_risk.json` e `normalization.json` e emite `references.i8.bin` (3M × 16 bytes) e `labels.bits` (3M / 8 = 375 KB). Roda apenas no estágio builder do Dockerfile.
- **`crates/api`** *(binário)* — servidor Axum com `POST /fraud-score` e `GET /ready`. Executa o scan AVX2, mantém top-5 e responde a decisão.
- **`infra/haproxy/haproxy.cfg`** — configuração mínima: 1 frontend HTTP em `:9999`, 1 backend `balance roundrobin` com `api-1:8080` e `api-2:8080`. Sem ACL, sem inspeção de payload.
- **`docker-compose.yml`** — declara serviço `lb` (HAProxy), `api-1`, `api-2` e o volume read-only `dataset-vol` populado a partir do estágio builder. Limites: `lb 0.10 / 30 MB`, `api-* 0.45 / 160 MB` cada (total 1.00 CPU / 350 MB).
- **`Dockerfile`** *(multi-stage)* — estágio `builder` compila `build-dataset` + `api` (release, `target-cpu=x86-64-v3`); estágio `dataset` roda `build-dataset` para gerar artefatos; estágio `runtime` (`gcr.io/distroless/cc-debian12`) recebe binário `api` + artefatos.
- **`info.json`** e branch `submission` — pacote de submissão conforme SUBMISSAO.md.

Fluxo de dados de uma request:

```
client ─POST /fraud-score─▶ HAProxy :9999 ─round-robin─▶ api-N :8080
                                                            │
                                                            ▼
                              [serde_json deserialize] ─▶ [normalize+quantize → [i8;16]]
                                                            │
                                                            ▼
                              [AVX2 L2² scan vs &[i8] mmap] ─▶ [top-5 buffer]
                                                            │
                                                            ▼
                              [labels.bits lookup ×5] ─▶ [fraud_score = #fraudes/5]
                                                            │
                                                            ▼
                              [serde_json serialize {approved, fraud_score}] ─▶ HTTP 200
```

## Design de Implementação

### Interfaces Principais

```rust
// crates/shared/src/lib.rs
pub const DIMS: usize = 14;
pub const PAD: usize = 16;            // alinhamento AVX2
pub const SENTINEL_I8: i8 = -1;       // last_transaction == null

pub struct Normalization { /* serde de normalization.json */ }
pub struct McсRisk(pub HashMap<String, f32>);  // default 0.5

pub fn vectorize(payload: &Payload, n: &Normalization, mcc: &McсRisk) -> [f32; DIMS];
pub fn quantize(v: &[f32; DIMS]) -> [i8; PAD];          // [0.0,1.0]→[0,100]; sentinela→-1
pub fn dataset_byte_len(n: usize) -> usize { n * PAD }  // 3M × 16 = 48 MB

// crates/api/src/search.rs
pub struct Top5 { dist: [i32; 5], idx: [u32; 5] }       // ordenado ascendente
pub fn knn5_avx2(query: &[i8; PAD], refs: &[i8]) -> Top5;  // unsafe: target_feature avx2
pub fn fraud_score(top: &Top5, labels: &[u8]) -> f32;   // popcount sobre labels.bits

// crates/api/src/state.rs
pub struct AppState {
    pub ready: AtomicBool,
    pub refs: Mmap,                     // &[i8] após cast
    pub labels: Mmap,                   // &[u8]
    pub norm: Arc<Normalization>,
    pub mcc:  Arc<McсRisk>,
}
```

### Modelos de Dados

**Layout do `references.i8.bin`** — array `C` contíguo de `3_000_000` × `[i8; 16]` (14 dimensões + 2 bytes de padding zerados). Total: 48 MB. Magic header de 16 bytes: `b"RINHA26\x01" + u64_le(N)`.

**Layout do `labels.bits`** — bitset little-endian de 1 bit por vetor: `1` = fraud, `0` = legit. 3M / 8 = 375.000 bytes.

**Request/Response** (mirroring API.md):

```rust
#[derive(Deserialize)]
struct Payload<'a> {
    id: &'a str,
    transaction: Tx,
    customer: Customer<'a>,
    merchant: Merchant<'a>,
    terminal: Terminal,
    last_transaction: Option<LastTx>,
}
#[derive(Serialize)]
struct Resp { approved: bool, fraud_score: f32 }
```

### Endpoints de API

- **`POST /fraud-score`** — body JSON conforme API.md; resposta `200 {approved, fraud_score}`. Em payload inválido, `400` com body vazio. Nunca propaga `5xx` — qualquer falha do scan resolve para resposta determinística (ver "Riscos").
- **`GET /ready`** — `200` quando `AppState.ready == true`; `503` antes. Habilitado depois do `mmap` carregado, do parse dos JSONs e do touch da primeira página de `refs` (warmup).

### Decisão e Quantização

Detecção determinística:

1. `vectorize()` aplica clamp `[0.0,1.0]`; `last_transaction:null` deixa dims 5 e 6 como `f32::NAN` para flag interna.
2. `quantize()` mapeia cada dim por `(x * 100.0).round() as i8` (range `[0,100]`) ou `-1` para a sentinela. Padding 14 e 15 = `0`.
3. `knn5_avx2()` calcula L2² (sem `sqrt` — monotônico, preserva ordem). Loop de batch processa 16 vetores por iteração: carrega 16×16 bytes consecutivos, faz `_mm256_sub_epi8` contra a query broadcast, `_mm256_madd_epi16` após `_mm256_cvtepi8_epi16`, acumula em `__m256i` `i32`. Cada iteração emite 16 distâncias e atualiza o top-5.
4. **Tie-break determinístico**: ordenar `(dist, idx)` ascendente — distâncias iguais ficam na ordem do índice no dataset, garantindo paridade entre execuções.
5. `fraud_score = popcount(labels[idx_i] for i in 0..5) / 5.0`; `approved = fraud_score < 0.6`.

Erro de quantização teórico: pior caso por dim = 0,005 → distância máxima distorcida ≤ √(14·0,01) ≈ 0,12. Validado offline contra oráculo `f32` em ≥10 mil amostras (ver Testes).

### Threading e Runtime

Cada API roda em `tokio::runtime::Builder::new_current_thread().enable_all().build()`. O handler é `async fn` mas o scan é uma chamada bloqueante de ~0,3–0,5 ms; com 0,45 CPU efetivo e payload sequencial do k6, current_thread evita context-switch e mantém p99 estável. Não é necessário `spawn_blocking` — a duração do scan é menor que o slice de scheduling padrão.

`AppState` é `Arc<…>` clonado em cada handler. Os `Mmap` são `Send + Sync` e expostos como `&[i8]` via `unsafe { from_raw_parts }`.

## Pontos de Integração

Sem integrações externas em runtime. Em build, o estágio `dataset` consome `resources/references.json.gz`, `resources/mcc_risk.json` e `resources/normalization.json` do repositório (já presentes neste workspace). Imagens externas:

- `gcr.io/distroless/cc-debian12` (runtime das APIs) — pública, multi-arch.
- `haproxytech/haproxy-alpine:2.9` (LB) — pública, ~10 MB, amd64.

Build cross-arch: `docker buildx build --platform linux/amd64` é obrigatório para mantenedores em Apple Silicon (FAQ.md). O Dockerfile fixa `--platform=linux/amd64` em todos os `FROM`.

## Abordagem de Testes

### Testes de Unidade

- **`shared::vectorize`** — golden cases tirados de REGRAS_DE_DETECCAO.md (transação legítima e fraudulenta) com vetor esperado bit-a-bit.
- **`shared::quantize`** — propriedades: clamp em `[0,100]`, sentinela preservada, idempotência sob roundtrip `f32 → i8 → f32_estimado`.
- **`api::search::knn5_avx2`** — comparação com implementação naïve em `f32` sobre 100k vetores aleatórios (subset do dataset). Critério: 100% de paridade nos 5 vizinhos para ≥99,5% das queries; mismatches restantes documentados como erro de quantização aceitável (failure budget 0,5%).
- **Tie-break** — caso construído com 6 vetores distintos a distância idêntica; verificar que os 5 retornados são os de menor índice.

### Testes de Integração

- `cargo test` com `tokio::test` levanta a API em porta efêmera, monta `mmap` de um dataset reduzido (10 mil vetores) e exercita `/ready` (espera 200) seguido de 200 requisições em `/fraud-score` cobrindo: caminho feliz, `last_transaction:null`, MCC ausente, payload com campos extra, payload inválido (400).
- Smoke `docker-compose up` local: healthcheck `/ready` em ambos `api-1` e `api-2`, depois `oha -n 5000 -c 50 http://localhost:9999/fraud-score` para confirmar p99 ≤ 1 ms no Mac Mini Late 2014 (ou perfil equivalente).

### Testes de E2E

Não se aplica — não há frontend. O E2E real é o k6 da Rinha, executado pela engine via issue `rinha/test`. Manter o script `test/` do repositório oficial como ferramenta de iteração local (fora do entregável).

## Sequenciamento de Desenvolvimento

### Ordem de Construção

1. **`crates/shared`** primeiro — vetorização e quantização são pré-requisito de tudo; permite criar testes golden antes de qualquer infra.
2. **`crates/build-dataset`** — gera artefatos em `target/dataset/` para uso local; só depende de `shared`.
3. **`crates/api`** com `/ready` + scan single-threaded (sem SIMD, em `f32`) — caminho funcional ponta-a-ponta para validar contrato HTTP e paridade com oráculo.
4. **Kernel AVX2** — substitui o scan naïve, mede ganho de p99 com `oha`/`wrk` localmente.
5. **Dockerfile multi-stage + docker-compose.yml + HAProxy** — empacota tudo, valida footprint de CPU/memória dentro de 1.0/350 MB.
6. **Branch `submission` + `info.json`** — última etapa, depois do `final_score` local convergir.

### Dependências Técnicas

- Toolchain Rust estável (edição 2024) com `target-cpu=x86-64-v3` (AVX2 + FMA garantido).
- Docker 24+ com `buildx` para build cross-arch em Apple Silicon.
- Acesso de leitura aos arquivos em `resources/` (já presentes no repo).

## Monitoramento e Observabilidade

Escopo mínimo (PRD declara observability avançada como fora de escopo):

- Logs `tracing_subscriber` em formato single-line com nível `info` no boot (`api ready in {ms}ms, refs={N}`) e `warn`/`error` em falhas de boot. Logs em inglês (PRD Persona Secundária).
- Sem métricas Prometheus, sem tracing distribuído, sem dashboards.
- `/ready` é a única superfície de saúde. Em produção (durante o teste) os logs vão para `stdout` do container e podem ser inspecionados via `docker logs` em caso de falha.

## Considerações Técnicas

### Decisões Principais

| Decisão | Escolha | Justificativa |
|---|---|---|
| Pré-processamento | Build-time (multi-stage) | `/ready` em <100 ms; sem rede no boot; reprodutibilidade. |
| SIMD | `std::arch` AVX2 (estável) | Mac Mini 2014 (Haswell) suporta AVX2; sem nightly. |
| Imagem runtime | `distroless/cc-debian12` | ~20 MB, glibc, sem shell. RSS ocioso mínimo. |
| Split de recursos | `0.10/30 + 0.45/160 + 0.45/160` | Folga para HAProxy; 160 MB cabe mmap (~48 MB) + heap + page cache compartilhado. |
| Runtime Tokio | `current_thread` + scan inline | Sem jitter de work-stealing; 0,45 CPU não justifica 2 workers. |
| Quantização | `[0,1] → [0,100]`, sentinela `-1` | Sem overflow no acumulador `i32`; erro de quantização << 0,5%. |
| JSON | `serde_json` zero-copy | Payload pequeno; ganho de simd-json marginal. |
| Top-k | Buffer fixo `[i32;5]` + insertion sort | Mais rápido que `BinaryHeap` para k=5. |

### Riscos Conhecidos

- **Erro de quantização > 0,5%** — mitigação: oráculo `f32` em CI valida paridade em 10k amostras; se falhar, aumentar resolução para `[0,127]` (custa overflow check).
- **Page faults sob carga** — primeira request paga ~50 MB de page-in. Mitigação: warmup explícito no boot (touch sequencial de `refs` + `MADV_WILLNEED`) antes de marcar `ready=true`.
- **Build cross-arch quebrar em Apple Silicon** — mitigação: documentar `docker buildx build --platform linux/amd64` no README; CI com matrix `linux/amd64`.
- **HAProxy ser gargalo a 5k req/s** — improvável (round-robin TCP custa <0,1 ms), mas se ocorrer migrar para `nginx` ou aumentar `0.10` → `0.15` CPU realocando do api.
- **Tie-break não-determinístico** — risco de divergir do oráculo em 5º vizinho; mitigação: ordenação estável `(dist, idx)` documentada e testada.

### Conformidade com Skills Padrões

Este repositório não possui pasta `.claude/skills/` local. As skills globais aplicáveis a este tech spec:

- **`rust-best-practices`** — diretrizes de ownership, error handling, async, traits, clippy, testes.
- **`systematic-debugging`** — para investigar divergências contra oráculo.
- **`extreme-software-optimization`** — profiling-driven optimization quando p99 não saturar em 1 ms.
- **`verification-before-completion`** — confirmar `cargo build --release`, `cargo clippy -- -D warnings`, `cargo test` e medição local (`oha`) antes de declarar pronto.
- **`no-workarounds`** — qualquer divergência da paridade com o oráculo deve ser fixada na quantização/tie-break, não com hacks no caminho de decisão.

### Arquivos Relevantes e Dependentes

- `Cargo.toml` (workspace) e `Cargo.toml` por crate.
- `crates/shared/src/{lib.rs,vectorize.rs,quantize.rs,format.rs}`.
- `crates/build-dataset/src/main.rs`.
- `crates/api/src/{main.rs,routes.rs,search.rs,state.rs,error.rs}`.
- `infra/haproxy/haproxy.cfg`.
- `Dockerfile` (multi-stage), `docker-compose.yml`.
- `info.json` (branch `submission`).
- `resources/{references.json.gz,mcc_risk.json,normalization.json}` (entrada do build).
- `tasks/prd-rinha-backend-2026/prd.md` (escopo funcional).
- Documentos de referência: `API.md`, `ARQUITETURA.md`, `REGRAS_DE_DETECCAO.md`, `DATASET.md`, `AVALIACAO.md`, `SUBMISSAO.md`, `FAQ.md`.
