# PRD — Submissão Rinha de Backend 2026 (Detecção de Fraude por Busca Vetorial em Rust)

## Visão Geral

A Rinha de Backend 2026 desafia participantes a construir um módulo de detecção de fraude que, para cada transação de cartão recebida, devolva uma decisão binária (`approved`) e um `fraud_score`. A decisão é apoiada por uma busca vetorial: a transação é convertida num vetor de 14 dimensões e comparada contra um dataset de 3.000.000 vetores rotulados como `fraud` ou `legit`.

Este PRD descreve o produto a ser entregue: uma submissão completa em Rust, composta por um load balancer HAProxy, duas instâncias de uma API Rust acopladas a artefatos pré-processados em volume compartilhado e os arquivos de submissão exigidos pela organização. O objetivo é disputar pódio (top 3) maximizando a pontuação oficial (latência + qualidade de detecção, faixa de -6000 a +6000).

O valor está em demonstrar uma solução ao mesmo tempo correta (decisão idêntica ao oráculo k-NN exato com k=5 e distância euclidiana) e extremamente eficiente dentro do orçamento apertado de 1 CPU e 350 MB para todos os serviços somados.

## Objetivos

- **Métrica primária — `final_score` máximo**: alcançar `final_score ≥ 5800` (de um teto de +6000) no teste oficial, com ambos os componentes acima de +2900.
- **Latência (`score_p99` ≥ 2900)**: p99 observado pelo k6 ≤ 1 ms na prévia oficial e no teste final, jamais ultrapassando 10 ms sob carga incremental.
- **Detecção (`score_det` ≥ 2900)**: `failure_rate` (FP + FN + Err / N) ≤ 0,5%; respostas idênticas ao oráculo (k-NN exato euclidiano com k=5) sempre que numericamente possível dentro do esquema de quantização adotado.
- **Disponibilidade**: zero erros HTTP (`Err = 0`) ao longo do teste.
- **Conformidade de infra**: soma de limites de CPU ≤ 1 e memória ≤ 350 MB no `docker-compose.yml`; rede `bridge`; porta `9999`; imagens `linux-amd64` públicas.
- **Reprodutibilidade**: construir, executar e medir a solução localmente com um único comando, e abrir issue `rinha/test` recebendo resultado válido em até 5 tentativas de prévia.

## Histórias de Usuário

- **Como Engine da Rinha (k6)**, eu envio `POST /fraud-score` em sequência incremental para que o backend responda rapidamente com a decisão correta, alimentando o cálculo de p99 e da matriz de confusão.
- **Como Engine da Rinha**, eu chamo `GET /ready` antes do teste para que só inicie a carga quando a API estiver com dataset carregado.
- **Como avaliador da Rinha**, eu abro a issue `rinha/test` no repositório do participante para que a engine execute o teste oficial e poste o resultado.
- **Como participante mantenedor da submissão**, eu rodo o teste localmente várias vezes para iterar nos perfis de build até atingir a meta de pontuação.
- **Como leitor da comunidade**, eu navego no código da branch `main` para entender a estratégia adotada e aprender com a solução.
- **Como organização da Rinha**, eu inspeciono `info.json` e o `docker-compose.yml` para validar conformidade com as regras (≤ 1 CPU / 350 MB, `bridge`, porta `9999`, LB sem lógica de detecção).

### Casos extremos cobertos

- Transação com `last_transaction: null` deve resultar em vetor com sentinela `-1` nas posições 5 e 6.
- MCC ausente em `mcc_risk.json` deve usar valor padrão `0.5`.
- Valores extrapolando os tetos de normalização devem ser limitados ao intervalo `[0.0, 1.0]` via clamp.
- Empates de distância nos vizinhos mais próximos devem ser resolvidos de forma determinística para garantir reprodutibilidade.

## Funcionalidades Principais

### F1. Endpoint `POST /fraud-score` (decisão de fraude)

Receber payload JSON com `transaction`, `customer`, `merchant`, `terminal` e `last_transaction` (opcional), produzir um vetor de 14 dimensões e responder com `approved` e `fraud_score` no formato definido em [API.md](../../API.md).

**Requisitos funcionais:**
1. A API DEVE aceitar requisições `POST` em `/fraud-score` com `Content-Type: application/json`.
2. A API DEVE devolver `HTTP 200` com corpo `{ "approved": <bool>, "fraud_score": <float> }`.
3. A API DEVE calcular `fraud_score = número_de_fraudes_entre_os_5_vizinhos / 5`.
4. A API DEVE definir `approved = (fraud_score < 0.6)`, com threshold fixo em `0.6`.
5. A vetorização DEVE seguir as 14 dimensões e fórmulas de normalização especificadas em [REGRAS_DE_DETECCAO.md](../../REGRAS_DE_DETECCAO.md), incluindo clamp em `[0.0, 1.0]` e sentinela `-1` para `last_transaction: null`.
6. A busca DEVE retornar os 5 vizinhos mais próximos por distância euclidiana sobre as 14 dimensões, com resultados idênticos aos do oráculo (k-NN exato com k=5).
7. Empates de distância DEVEM ser resolvidos de forma determinística (regra estável definida na implementação).
8. Em caso de payload inválido, a API DEVE responder com `HTTP 400` e nunca propagar `HTTP 5xx` originados pela busca.

### F2. Endpoint `GET /ready` (prontidão)

Verificação de prontidão consumida pela engine antes do início do teste.

**Requisitos funcionais:**
9. A API DEVE expor `GET /ready` na mesma porta interna que `/fraud-score`.
10. `GET /ready` DEVE responder `HTTP 200` somente após os artefatos de referência estarem totalmente carregados em memória/`mmap` e os pools de threads inicializados.
11. Antes de pronta, a API DEVE responder `HTTP 503` em `/ready` (sem mascarar com `200`).

### F3. Pré-processamento offline do dataset

Transformar `references.json.gz`, `mcc_risk.json` e `normalization.json` em artefatos binários compactos consumidos pela API em runtime.

**Requisitos funcionais:**
12. O processo de build DEVE gerar `references.i8.bin` (vetores quantizados em `i8`) e `labels.bits` (1 bit por vetor) a partir de `references.json.gz`.
13. As fórmulas de normalização aplicadas no pré-processamento DEVEM ser idênticas às aplicadas em runtime.
14. Os artefatos pré-processados DEVEM caber confortavelmente no orçamento de memória combinado (target: ~42 MB para os vetores + < 1 MB para os labels).
15. Os artefatos DEVEM ser distribuídos via volume read-only compartilhado entre as duas instâncias da API.

### F4. Topologia com load balancer e duas instâncias

Composição do `docker-compose.yml` segundo as regras de [ARQUITETURA.md](../../ARQUITETURA.md).

**Requisitos funcionais:**
16. O `docker-compose.yml` DEVE declarar pelo menos um load balancer (HAProxy) e exatamente duas instâncias da API Rust.
17. O LB DEVE distribuir requisições em **round-robin simples**, sem inspecionar payload, transformar corpo, aplicar condicionais ou responder antes de repassar.
18. O LB DEVE expor a porta `9999` para o cliente externo.
19. A soma de `cpus` e `memory` em `deploy.resources.limits` de todos os serviços DEVE ser `≤ 1.0` CPU e `≤ 350 MB`.
20. A rede do compose DEVE usar driver `bridge`; modos `host` e `privileged` SÃO PROIBIDOS.
21. Todas as imagens referenciadas DEVEM ser públicas e compatíveis com `linux-amd64`.

### F5. Pacote de submissão e fluxo da issue `rinha/test`

Estrutura do repositório seguindo [SUBMISSAO.md](../../SUBMISSAO.md).

**Requisitos funcionais:**
22. O repositório DEVE manter pelo menos duas branches: `main` (código-fonte completo) e `submission` (apenas artefatos para o teste, sem código-fonte).
23. A branch `submission` DEVE conter `docker-compose.yml` na raiz e `info.json`.
24. `info.json` DEVE conter `participants`, `social`, `source-code-repo`, `stack` e `open_to_work`.
25. O participante DEVE ser capaz de abrir issue com `rinha/test` na descrição e receber resposta automática da engine.

### F6. Restrição de uso de dados

26. A solução NÃO PODE usar payloads do teste como dataset de referência ou para lookup de fraudes (apenas `references.json.gz` é permitido como dataset).

## Experiência do Usuário

### Persona principal — Engine da Rinha (cliente automatizado)

A engine é um cliente k6 sem interface gráfica que precisa de respostas rápidas, corretas e estáveis. Sua "experiência" boa significa: `GET /ready` retorna `200` antes do início do teste; cada `POST /fraud-score` responde com latência consistentemente baixa; nenhuma resposta retorna `5xx`.

**Fluxo principal (caminho feliz):**

1. Engine sobe o `docker-compose.yml` da branch `submission` no Mac Mini (Late 2014, 2.6 GHz, 8 GB RAM, Ubuntu 24.04).
2. Engine faz polling em `GET http://localhost:9999/ready` até receber `200`.
3. Engine inicia carga incremental de `POST http://localhost:9999/fraud-score`.
4. Para cada requisição, HAProxy escolhe `api-1` ou `api-2` em round-robin; a instância vetoriza, busca top-5 e devolve `{ "approved", "fraud_score" }`.
5. Engine compara cada resposta com o rótulo esperado, registra TP/TN/FP/FN/Err e calcula p99.
6. Engine posta o resultado em comentário na issue `rinha/test` e fecha a issue.

### Persona secundária — leitor do código (comunidade)

Lê o código na `main` para aprender. A "experiência" boa é encontrar README claro com diagrama de topologia, instruções de build/run, e estrutura de diretórios autoexplicativa.

### Considerações de UI/UX e acessibilidade

- Não há UI humana; portanto, não se aplicam diretrizes de acessibilidade visual (WCAG).
- A "interface" são os contratos JSON e o `docker-compose.yml`. Eles DEVEM ser estáveis, autoconsistentes e idênticos aos exemplos da especificação.
- Mensagens de log devem ser concisas e em inglês para servir como diagnóstico em caso de falha durante o teste oficial.

## Restrições Técnicas de Alto Nível

- **Tecnologia obrigatória — Rust**: a API e o pipeline de pré-processamento DEVEM ser escritos em Rust (edição 2024, toolchain estável, build `--release`). Esta é uma restrição de produto: o objetivo é demonstrar Rust no podium da Rinha 2026.
- **Estratégia de busca obrigatória — scan linear exato com SIMD**: a busca DEVE ser k-NN exato por scan linear vetorizado sobre os 3M de vetores `i8`, paralelizado por threads. ANN aproximado e bancos vetoriais externos NÃO são permitidos como caminho principal.
- **Orçamento de recursos**: soma `≤ 1.0` CPU e `≤ 350 MB` para todos os serviços do compose juntos.
- **Rede**: modo `bridge` (sem `host`, sem `privileged`). Toda exposição externa pela porta `9999` única.
- **Imagens**: públicas, `linux-amd64`. Atenção especial ao build cross-arch para mantenedores em Apple Silicon.
- **LB sem lógica**: HAProxy em modo round-robin puro. Nenhuma inspeção de payload, condicional ou transformação.
- **Mínimo 2 instâncias da API** atrás do LB.
- **Performance**: p99 alvo ≤ 1 ms no Mac Mini Late 2014 do ambiente de teste oficial.
- **Conformidade de detecção**: paridade com oráculo k-NN exato (k=5, distância euclidiana sobre 14 dimensões) — qualquer divergência DEVE ser justificada e mensurada (ex.: erro de quantização `i8` aceitável se mantiver `failure_rate ≤ 0,5%`).
- **Privacidade/integridade**: dataset de referência é o único insumo de lookup; é PROIBIDO usar payloads do teste como referência ou cache.
- **Submissão**: estrutura de branches (`main` + `submission`) e arquivos (`docker-compose.yml`, `info.json`) seguindo [SUBMISSAO.md](../../SUBMISSAO.md).

Os trade-offs entre p99 e detecção devem ser resolvidos pela meta de **maximizar `score_final = score_p99 + score_det`**, tratando os cortes (15% de falhas, p99 > 2000 ms) como invioláveis.

Detalhes de design (escolha de framework HTTP, kernel SIMD específico, layout do `mmap`, perfis de compilação) ficam para a Tech Spec.

## Fora de Escopo

- ANN aproximado (HNSW, IVF, LSH) ou bancos vetoriais externos (pgvector, Qdrant, etc.) como caminho principal.
- Treinamento de modelo de Machine Learning (a solução é instance-based, não paramétrica).
- Persistência de transações recebidas em runtime (cada requisição é stateless).
- Autenticação, autorização ou rate-limiting na API.
- Observability avançada (tracing distribuído, dashboards, métricas Prometheus). Logs mínimos são suficientes.
- Suite de medição automatizada com k6/oha empacotada no entregável (medição local fica como ferramenta de desenvolvimento, fora do `docker-compose.yml` oficial).
- Suporte a outras arquiteturas além de `linux-amd64`.
- Endpoints adicionais além de `POST /fraud-score` e `GET /ready`.
- Internacionalização ou UI humana.
- Estratégias defensivas que usem payloads do teste como dataset (proibido pelas regras).
- Otimizações abaixo de 1 ms de p99 (não rendem pontos adicionais — score satura).

(Notas: riscos de implementação técnica, escolhas de bibliotecas, perfis de build e detalhes de paralelismo/SIMD serão tratados na Tech Spec.)
