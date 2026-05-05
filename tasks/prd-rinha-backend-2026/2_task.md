# Tarefa 2.0: Crate `api` — Axum + k-NN (scan naïve → AVX2) com paridade ao oráculo

<critical>Ler os arquivos de prd.md e techspec.md desta pasta, se você não ler esses arquivos sua tarefa será invalidada</critical>

## Visão Geral

Implementar o servidor HTTP em Axum sobre `tokio::current_thread`, com `mmap` dos artefatos gerados pela tarefa 1.0, endpoint `GET /ready` e endpoint `POST /fraud-score` retornando `{approved, fraud_score}`. A busca é desenvolvida em duas subetapas dentro desta mesma tarefa para preservar o gate de qualidade definido na techspec: primeiro um **scan linear naïve em `f32`** (single-thread, sem SIMD) que valida o contrato HTTP e a paridade com o oráculo k-NN exato; depois o **kernel AVX2 `knn5_avx2`** que substitui o scan naïve mantendo paridade ≥ 99,5% e p99 ≤ 1 ms.

<skills>
### Conformidade com Skills Padrões

- **`rust-best-practices`** — async/await, error handling, `unsafe` minimizado e isolado, traits idiomáticas.
- **`extreme-software-optimization`** — profiling-driven para garantir p99 ≤ 1 ms e medir o ganho do kernel AVX2.
- **`systematic-debugging`** — para diagnosticar divergências de paridade entre AVX2 e a versão `f32`.
- **`verification-before-completion`** — confirmar `cargo test`, `cargo clippy -- -D warnings` e medição local com `oha` antes de declarar pronto.
- **`no-workarounds`** — qualquer divergência de paridade é resolvida na quantização/tie-break, não com hacks na decisão.
</skills>

<requirements>
- Servidor Axum em `tokio::runtime::Builder::new_current_thread().enable_all().build()`.
- `AppState` com `Mmap` dos artefatos (`memmap2`), `Arc<Normalization>`, `Arc<MccRisk>` e `AtomicBool ready`.
- Warmup explícito no boot: `MADV_WILLNEED` + touch sequencial das páginas de `refs` antes de marcar `ready=true`.
- `GET /ready` responde `503` antes do warmup e `200` depois; nunca mascara com `200` prematuramente.
- `POST /fraud-score` aceita `Content-Type: application/json`, devolve `200 {approved: bool, fraud_score: f32}`.
- `fraud_score = popcount(labels[idx_i] for i in 0..5) / 5.0`; `approved = fraud_score < 0.6`.
- Tie-break determinístico: ordenação estável `(dist, idx)` ascendente.
- Top-5 implementado com buffer fixo `[i32; 5]` + insertion sort (não usar `BinaryHeap`).
- Payload inválido → `HTTP 400` com body vazio. Nenhum `5xx` jamais propagado.
- Subetapa A (scan naïve `f32`): caminho funcional ponta-a-ponta, paridade 100% com oráculo k-NN exato em ≥ 10 mil amostras.
- Subetapa B (kernel AVX2): substituição do scan naïve, `unsafe` isolado em função com `#[target_feature(enable = "avx2")]`, batch de 16 vetores/iteração via `_mm256_sub_epi8` → `_mm256_cvtepi8_epi16` → `_mm256_madd_epi16` → acumulador `i32`. Paridade ≥ 99,5% contra a versão naïve em 10 mil amostras.
- Logs `tracing_subscriber` em inglês: linha única no boot (`api ready in {ms}ms, refs={N}`).
- Sem `spawn_blocking` no caminho da request (scan inline na task).
</requirements>

## Subtarefas

- [x] 2.1 Criar crate binário `api` com Axum + Tokio current_thread; estrutura `AppState` e `mmap` dos artefatos via `memmap2`.
- [x] 2.2 Implementar warmup (`MADV_WILLNEED` + touch sequencial) e endpoint `GET /ready` (503 → 200 pós-warmup).
- [x] 2.3 Implementar `POST /fraud-score` com deserialize zero-copy `serde_json`, vetorização/quantização via crate `shared` e resposta `{approved, fraud_score}`.
- [x] 2.4 Implementar scan linear naïve `f32` single-thread sobre `&[i8]` (L2² sem `sqrt`), top-5 com `[i32; 5]` + insertion sort e tie-break `(dist, idx)`.
- [x] 2.5 Validar paridade do scan naïve contra oráculo k-NN exato em 10 mil amostras (100%).
- [x] 2.6 Implementar `knn5_avx2` (kernel `unsafe` com `target_feature(enable = "avx2")`, batch de 16 vetores) substituindo o scan naïve.
- [x] 2.7 Validar paridade do AVX2 vs naïve em 10 mil amostras (≥ 99,5%) — bit-exact 100% via Rosetta. Medição de p99 com `oha` requer dataset de 3M (gerado pela tarefa 3.0/Docker), portanto fica para empacotamento.
- [x] 2.8 Tratamento de erro: payload inválido → `HTTP 400`; nenhum `5xx` propagado.
- [x] 2.9 Logs `tracing` em inglês no boot; rodar `cargo test`, `cargo clippy -- -D warnings`.

## Detalhes de Implementação

Ver seções "Interfaces Principais", "Decisão e Quantização", "Threading e Runtime", "Endpoints de API" e "Riscos Conhecidos" da [techspec.md](./techspec.md). Em particular: o `unsafe` do kernel AVX2 deve ficar isolado dentro de uma função `#[target_feature(enable = "avx2")]` e ser exercido apenas pelo handler; a quantização do query no caminho da request DEVE usar exatamente as mesmas funções de `shared` que o `build-dataset` usa.

## Critérios de Sucesso

- `GET /ready` retorna `503` antes do warmup e `200` depois — verificável em teste de integração.
- `POST /fraud-score` retorna respostas idênticas ao oráculo k-NN exato (paridade 100% com a versão `f32`; ≥ 99,5% após substituição pelo AVX2).
- `failure_rate` (FP + FN + Err / N) ≤ 0,5% no dataset de validação.
- p99 medido com `oha -n 5000 -c 50` ≤ 1 ms em hardware equivalente ao Mac Mini Late 2014 (ou conforme perfil de desenvolvimento).
- Zero respostas `5xx` em qualquer cenário (incluindo payload malformado, ausência de campos, MCC ausente).
- `cargo clippy -- -D warnings` e `cargo test --workspace` passam.

## Testes da Tarefa

- [ ] Testes de unidade — `knn5_avx2` comparado contra implementação naïve `f32` em 100k vetores aleatórios; tie-break com 6 vetores a distância idêntica (verificar que os 5 retornados são os de menor índice); `fraud_score` via popcount sobre labels.
- [ ] Testes de integração — `tokio::test` levanta a API em porta efêmera com dataset reduzido (10 mil vetores), aguarda `/ready=200` e exercita 200 requisições em `/fraud-score` cobrindo: caminho feliz, `last_transaction:null`, MCC ausente, payload com campos extra, payload inválido (400).
- [ ] Testes E2E — não se aplica nesta tarefa (E2E real é o k6 da Rinha; smoke local com `oha` cobre p99).

<critical>SEMPRE CRIE E EXECUTE OS TESTES DA TAREFA ANTES DE CONSIDERÁ-LA FINALIZADA</critical>

## Arquivos relevantes

- `crates/api/Cargo.toml`
- `crates/api/src/main.rs`
- `crates/api/src/routes.rs`
- `crates/api/src/state.rs`
- `crates/api/src/search.rs` (scan naïve + `knn5_avx2`)
- `crates/api/src/error.rs`
- `crates/shared/src/{vectorize.rs,quantize.rs,format.rs}` (consumidos)
- `target/dataset/references.i8.bin`, `target/dataset/labels.bits` (produzidos pela tarefa 1.0)
