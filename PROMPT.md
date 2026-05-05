# PROMPT — Participação na Rinha de Backend 2026

Este arquivo é o ponto de partida da nossa solução. Ele aponta para a especificação oficial do desafio e fixa a stack que vamos usar.

## Contexto do desafio

A Rinha de Backend 2026 pede uma API de **detecção de fraude por busca vetorial**: cada transação vira um vetor de 14 dimensões, buscamos os 5 vizinhos mais próximos no dataset de referência (3 M de vetores rotulados) e devolvemos `approved` + `fraud_score`.

A especificação completa está distribuída nos arquivos abaixo — leia nessa ordem:

1. [README.md](./README.md) — visão geral e roteiro de leitura.
2. [API.md](./API.md) — contrato dos endpoints `POST /fraud-score` e `GET /ready`.
3. [ARQUITETURA.md](./ARQUITETURA.md) — limites de infra (1 CPU / 350 MB no total, load balancer + 2 instâncias, `bridge`, porta `9999`).
4. [REGRAS_DE_DETECCAO.md](./REGRAS_DE_DETECCAO.md) — as 14 dimensões, fórmulas de normalização e exemplos.
5. [BUSCA_VETORIAL.md](./BUSCA_VETORIAL.md) — fundamento de busca vetorial.
6. [DATASET.md](./DATASET.md) — formato de `references.json.gz`, `mcc_risk.json`, `normalization.json`.
7. [AVALIACAO.md](./AVALIACAO.md) — fórmula de pontuação (latência + detecção, -6000 a +6000).
8. [SUBMISSAO.md](./SUBMISSAO.md) — fluxo de PR, branches `main` e `submission`, issue `rinha/test`.
9. [FAQ.md](./FAQ.md) — armadilhas comuns.

## Stack escolhida

Vamos com **Rust**, mirando o melhor uso possível dos limites de CPU/memória e o menor p99 dentro do orçamento.

| Camada | Escolha | Motivo |
|---|---|---|
| Linguagem / runtime | **Rust** (edição 2024, toolchain estável, build `--release` com `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`) | Sem GC, footprint mínimo e controle total sobre layout de memória — essencial com 350 MB no total. |
| Runtime async | **Tokio** (`rt-multi-thread`, com `worker_threads` ajustado à cota de CPU) | Padrão de fato para I/O assíncrono em Rust, integra com todo o ecossistema HTTP. |
| Framework HTTP | **Axum** (sobre `hyper` + `tower`) | API ergonômica, overhead desprezível por requisição, fácil de medir. |
| Serialização | **`serde` + `serde_json`** (ou `simd-json` se valer o ganho) | Parse/serialize do payload de `/fraud-score` sem alocar mais que o necessário. |
| Load balancer | **HAProxy** (round-robin, modo TCP) | Footprint pequeno, distribui entre as 2 instâncias sem aplicar lógica. |
| Dataset em memória | **Arquivo `i8` quantizado + `memmap2`** num volume compartilhado | Paga ~42 MB uma vez (3M × 14 em i8) e o kernel compartilha as páginas read-only entre as 2 instâncias. |
| Busca vetorial | **Scan linear sobre `&[i8]`** com kernel SIMD (`std::simd` / `packed_simd` / intrínsecos `x86_64` AVX2) e paralelismo via **`rayon`** | Com só 14 dimensões, scan linear vetorizado bate HNSW em memória/latência e cabe num único passe sequencial pelo `mmap`. |
| Top-k | Heap binário de capacidade 5 (`BinaryHeap` ou implementação manual) | Mantém os 5 melhores em O(n log 5) sem alocação por requisição. |
| Pré-processamento | Binário Rust separado (`xtask` ou `bin/build_dataset.rs`) que lê `references.json.gz` e gera `references.i8.bin` + `labels.bits` | Mesma linguagem do servidor, evita divergência de fórmulas de normalização/quantização. |

### Topologia (resumo)

```
        ┌───────────┐
client ─▶│  HAProxy  │── round-robin ──▶ api-1 (Rust + Axum)
        │   :9999   │                    api-2 (Rust + Axum)
        └───────────┘                       │
                                            ▼
                                ┌──────────────────────────┐
                                │ volume read-only          │
                                │  references.i8.bin (mmap) │
                                │  labels.bits (mmap)       │
                                │  mcc_risk.json            │
                                │  normalization.json       │
                                └──────────────────────────┘
```

## Plano de execução (alto nível)

1. **Pré-processamento (build offline):** binário Rust lê `references.json.gz`, normaliza conforme [REGRAS_DE_DETECCAO.md](./REGRAS_DE_DETECCAO.md), quantiza para `i8` e grava `references.i8.bin` + `labels.bits` (1 bit por vetor: `fraud`/`legit`). Esses artefatos vão num volume read-only montado nas duas APIs.
2. **Boot da API:** `mmap` (via `memmap2`) dos artefatos como `&[i8]` / `&[u8]`, parse de `mcc_risk.json` e `normalization.json` em estruturas `Arc<…>`, montagem do pool `rayon` limitado pela cota de CPU.
3. **`POST /fraud-score`:** validar payload com `serde`, montar vetor de 14 dimensões em buffer de pilha (`[i8; 14]`), particionar o slice de referências entre threads `rayon`, cada thread calcula distâncias com kernel SIMD e mantém um heap top-5 local; merge final de heaps, `fraud_score = fraudes/5`, resposta `approved = fraud_score < 0.6`.
4. **`GET /ready`:** responder `200` só depois do `mmap` carregado e do pool de threads inicializado (sinalizado por um `OnceCell`/`AtomicBool`).
5. **Submissão:** seguir [SUBMISSAO.md](./SUBMISSAO.md) — branch `main` com código, branch `submission` com `docker-compose.yml` + artefatos pré-processados.

## Restrições que não podemos violar

- Soma dos limites de CPU ≤ 1 e memória ≤ 350 MB **em todos os serviços somados**.
- Modo de rede `bridge` (sem `host`/`privileged`).
- Load balancer **não pode** aplicar lógica de detecção.
- Pelo menos 2 instâncias da API atrás do LB.
- Imagens públicas, `linux-amd64`.
- Proibido usar payloads do teste como referência.

## Próximos passos

- [ ] Esqueleto do projeto: workspace Cargo (`api/` + `xtask/` ou `bin/build_dataset.rs`), `Dockerfile` multi-stage (builder com `rust:slim`, runtime `debian:bookworm-slim` ou `gcr.io/distroless/cc`), `docker-compose.yml` com HAProxy + 2 APIs + volume compartilhado.
- [ ] Binário de pré-processamento (`build_dataset`) gerando `references.i8.bin` e `labels.bits`.
- [ ] Carregamento via `memmap2` + tipagem segura como `&[i8]`.
- [ ] Implementação do scan linear com kernel SIMD e paralelização via `rayon`, com heap top-5 por thread.
- [ ] Endpoint `POST /fraud-score` em Axum + `GET /ready` com flag de prontidão.
- [ ] Medir p99 local com `wrk`/`oha` e iterar nos perfis de build (`lto`, `codegen-units`, `target-cpu=native` se a imagem permitir).
