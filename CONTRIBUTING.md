# Contribuir

Comece pelo README e pelas limitações conhecidas. Para propor uma correção, descreva o comportamento esperado e um cenário que reproduza o defeito antes de ampliar o escopo.

- Preserve IDs, perfis, credenciais, grupos e autorizações durante atualizações.
- Não resolva erros de rede ou cofre exigindo um novo login por suposição.
- Testes de UI com dados falsos não substituem testes nativos de navegador, cofre ou processos.
- Mudanças de lifecycle precisam cobrir falha, cancelamento, nova abertura e encerramento.
- Use TypeScript estrito, `cargo fmt` e Clippy com warnings negados.
- Não envie instruções pessoais de agentes, memórias, notas privadas, arquivos de ambiente, credenciais, bancos ou perfis.
- Use dados sintéticos e respeite a licença dos componentes de terceiros.

Veja os comandos em [docs/development.md](docs/development.md). A publicação de uma release exige revisão do conteúdo do instalador, não apenas do Git.
