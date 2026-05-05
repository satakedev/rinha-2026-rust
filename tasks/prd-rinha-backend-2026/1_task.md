# Tarefa 1.0: Fundações Rust — workspace, crate `shared` e crate `build-dataset`

<critical>Ler os arquivos de prd.md e techspec.md desta pasta, se você não ler esses arquivos sua tarefa será invalidada</critical>

## Visão Geral

Estabelecer as fundações do projeto Rust: workspace Cargo (edição 2024) com perfil release otimizado para `x86-64-v3`, crate biblioteca `shared` (vetorização, quantização, parsers de configuração e formato binário dos artefatos) e crate binário `build-dataset` que consome os arquivos de `resources/` e gera `references.i8.bin` + `labels.bits`. Esta tarefa entrega tudo o que é pré-requisito para o crate `api`: tipos, fórmulas de detecção compartilhadas com o oráculo e os arquivos binários que serão `mmap`-eados em runtime.

<skills>
### Conformidade com Skills Padrões

- **`rust-best-practices`** — diretrizes de ownership, error handling, traits, clippy e testes.
- **`verification-before-completion`** — confirmar `cargo build --release`, `cargo clippy -- -D warnings` e `cargo test` antes de declarar pronto.
- **`no-workarounds`** — paridade com o oráculo é resolvida na vetorização/quantização, não com hacks.
- **`systematic-debugging`** — para investigar divergências de paridade contra o oráculo.
</skills>

<requirements>
- Workspace Cargo na raiz do repositório com edição 2024 e toolchain estável fixada via `rust-toolchain.toml`.
- Perfil `release` com `target-cpu=x86-64-v3` (AVX2 + FMA garantidos).
- Crate `shared` com `vectorize()`, `quantize()`, parsers de `normalization.json` e `mcc_risk.json` (default 0.5 para MCC ausente), e helpers de leitura/escrita do formato binário.
- Constantes públicas: `DIMS = 14`, `PAD = 16`, `SENTINEL_I8 = -1`.
- Header binário `b"RINHA26\x01" + u64_le(N)` em `references.i8.bin` (16 bytes).
- Layout: 3.000.000 × `[i8; 16]` (~48 MB) para vetores; 3M / 8 = 375 KB para `labels.bits` (1 = fraud, 0 = legit, little-endian).
- Crate `build-dataset` lê `resources/references.json.gz` em streaming gzip, reusa `shared` para vetorizar e quantizar, e emite os dois artefatos.
- Clamp `[0.0, 1.0]` aplicado em runtime e em build-time é IDÊNTICO (mesmo código compartilhado).
- `last_transaction: null` deve produzir sentinela `-1` nas dimensões 5 e 6 após `quantize()`.
- `cargo clippy -- -D warnings` passa sem warnings em todos os crates.
</requirements>

## Subtarefas

- [x] 1.1 Criar `Cargo.toml` raiz (workspace) e `rust-toolchain.toml` com toolchain estável; configurar perfil `release` com `target-cpu=x86-64-v3`.
- [x] 1.2 Criar crate `shared` (library): módulos `vectorize`, `quantize`, `format` e tipos `Normalization` / `MccRisk`.
- [x] 1.3 Implementar `vectorize()` (14 dimensões, clamp, sentinela NaN para `last_transaction:null`) e `quantize()` (`[0,1]→[0,100]` em `i8`, sentinela `-1`, padding zerado).
- [x] 1.4 Implementar parsers de `normalization.json` e `mcc_risk.json` com default 0.5 para MCC ausente.
- [x] 1.5 Implementar leitura/escrita do `references.i8.bin` (magic header + payload contíguo) e do `labels.bits` (bitset).
- [x] 1.6 Criar crate binário `build-dataset` que lê `references.json.gz` em streaming, vetoriza/quantiza usando `shared` e emite os artefatos em `target/dataset/`.
- [x] 1.7 Escrever testes de unidade golden (REGRAS_DE_DETECCAO.md) para `vectorize` e `quantize`.
- [x] 1.8 Escrever teste de integração que executa `build-dataset` num subset reduzido e valida tamanho/header dos artefatos.
- [x] 1.9 Rodar `cargo build --release`, `cargo clippy -- -D warnings` e `cargo test --workspace`.

## Detalhes de Implementação

Ver seções "Visão Geral dos Componentes", "Modelos de Dados", "Decisão e Quantização" e "Interfaces Principais" da [techspec.md](./techspec.md). Em particular: o layout `[i8; 16]` (14 dims úteis + 2 bytes de padding zerados) é obrigatório para alinhamento AVX2; a função `quantize()` deve ser idempotente sob roundtrip `f32 → i8 → f32_estimado`.

## Critérios de Sucesso

- Workspace compila em release sem warnings.
- `vectorize` + `quantize` produzem vetores bit-a-bit idênticos aos golden cases derivados de REGRAS_DE_DETECCAO.md.
- `build-dataset` gera `references.i8.bin` com tamanho exato `16 + 3_000_000 × 16` bytes e `labels.bits` com `375_000` bytes (validação de tamanho e magic header).
- Erro de quantização teórico (pior caso) ≤ 0,005 por dimensão, dentro do failure budget de 0,5%.

## Testes da Tarefa

- [ ] Testes de unidade — `shared::vectorize` golden cases (transação legítima e fraudulenta), `shared::quantize` (clamp em `[0,100]`, sentinela preservada, idempotência), parsers de JSON (MCC ausente → 0.5).
- [ ] Testes de integração — executar `build-dataset` sobre um subset reduzido (ex.: 10k vetores) e validar tamanho dos artefatos, magic header e roundtrip leitura/escrita.
- [ ] Testes E2E — não se aplica nesta tarefa.

<critical>SEMPRE CRIE E EXECUTE OS TESTES DA TAREFA ANTES DE CONSIDERÁ-LA FINALIZADA</critical>

## Arquivos relevantes

- `Cargo.toml` (workspace raiz)
- `rust-toolchain.toml`
- `crates/shared/Cargo.toml`
- `crates/shared/src/lib.rs`
- `crates/shared/src/vectorize.rs`
- `crates/shared/src/quantize.rs`
- `crates/shared/src/format.rs`
- `crates/build-dataset/Cargo.toml`
- `crates/build-dataset/src/main.rs`
- `resources/references.json.gz`, `resources/mcc_risk.json`, `resources/normalization.json` (entrada, já existentes)
