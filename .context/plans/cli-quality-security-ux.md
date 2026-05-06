# Plano incremental: testes, segurança, performance e UX do CLI

## Objetivo
Melhorar o CLI/TUI do Jcode em fatias verificáveis, sem tentar reescrever tudo de uma vez. Cada rodada deve entregar uma melhoria concreta, teste associado e evidência de build.

## Eixos
1. **Testes**
   - Ampliar testes unitários para helpers puros de UI, tema, animação, layout e formatação.
   - Cobrir fluxos críticos com debug socket: reload, streaming, running tool, rate limit, fila e modo shell.
   - Transformar bugs reproduzidos em regressões permanentes.

2. **Segurança**
   - Executar e fortalecer budgets: `check_panic_budget.py`, `check_swallowed_error_budget.py`, `security_preflight.sh`.
   - Revisar tool permissions, ações destrutivas, redaction de secrets e telemetry privacy.
   - Adicionar testes para sanitização/normalização quando houver superfície de entrada externa.

3. **Performance**
   - Medir antes/depois com scripts de startup, memória e TUI bench.
   - Evitar alocações por frame em paths de renderização quando helpers puros puderem ser cacheados ou isolados.
   - Respeitar reduced-motion e FPS configurado, com limites seguros.

4. **Interface, design e animações**
   - Padronizar animações em `jcode-tui-style` para serem testáveis e reutilizáveis.
   - Melhorar estados visuais de processamento, ferramentas, batch, streaming e alertas.
   - Manter fallback estático em modo reduced-motion.

## Primeira fatia implementada
- Endurecer cálculo de spinner contra `NaN`, FPS inválido e tempos negativos.
- Extrair barras animadas de ferramenta para `jcode-tui-style::theme::tool_activity_bars`.
- Melhorar visual de ferramenta em execução com trilha simétrica de 5 células (`●`, `◆`, `·`).
- Adicionar testes unitários para timing inválido, reduced-motion e simetria da barra.

## Critérios de aceite desta rodada
- `cargo test -p jcode-tui-style`
- `cargo check -p jcode`
- scripts de segurança/budget relevantes executados quando não excederem o tempo da rodada
- commit com plano e implementação

## Próximas rodadas sugeridas
1. Snapshot/debug-socket do status `RunningTool` para validar rendering real.
2. Refatorar outros indicadores inline para helpers testáveis.
3. ~~Adicionar preflight composto que roda budgets de panic, swallowed errors, dependency boundaries e testes de tema.~~ Implementado em `scripts/cli_quality_preflight.sh`.
4. Perfil de alocações/render frame em `draw_status` e idle animations.
5. Revisão de redaction em logs, telemetry e tool outputs.

## Segunda fatia implementada
- Adicionar `scripts/cli_quality_preflight.sh` como gate rápido e reproduzível para CLI/TUI.
- O preflight roda formatação, `check_panic_budget.py`, `check_swallowed_error_budget.py`, `check_dependency_boundaries.py`, `cargo test -p jcode-tui-style` e `cargo check -p jcode`.
- Como o orçamento de swallowed errors ainda tem dívida histórica ampla, o modo padrão reporta a falha como warning sem mascarar o output. Use `--strict-swallowed` para transformar essa etapa em bloqueio quando a dívida for reduzida ou ratchetada intencionalmente.
- Use `--check` em CI/validação limpa para não modificar formatação.
