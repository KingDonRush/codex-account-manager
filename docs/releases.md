# Releases e procedência

O primeiro snapshot público contém a fonte revisada da série alpha.8. Não há histórico Git autoral completo de todas as versões anteriores disponível neste snapshot.

Pacotes históricos só podem ser publicados depois de inspecionar o seu conteúdo, incluindo documentação e executáveis. Alguns artefatos de desenvolvimento podem embutir caminhos pessoais, instruções ou relatórios internos; esses originais não serão disponibilizados por conveniência.

Uma tag pública não deve fingir que a fonte atual é a fonte histórica exata. Releases de arquivo, quando disponíveis, precisam identificar sua procedência e limites. Uma recompilação pública precisa de nome, hash e descrição próprios, sem reutilizar silenciosamente a assinatura de um pacote diferente.

## Inventário revisado

[Catálogo público do histórico](https://github.com/KingDonRush/codex-account-manager/releases/tag/archive-inventory-2026-10-26): 11 versões e 20 artefatos identificados em [releases-inventory.json](releases-inventory.json). Os `.deb` e NSIS inspecionados continham caminhos pessoais de build e/ou documentação interna; os três AppImages também foram retidos, sem completar inspeção independente. Os originais não estão disponíveis para download neste repositório.

## Primeira distribuição pública

`v0.5.0-alpha.8-public.1` identifica uma entrega recompilada da fonte pública,
com documentação revisada e caminhos de compilação neutros. A versão interna
do aplicativo permanece alpha.8. Não é uma renomeação dos antigos instaladores,
uma nova homologação do login ou uma correção das limitações conhecidas do Windows.
