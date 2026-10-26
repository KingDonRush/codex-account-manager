# API local v2

Consuma `codex-account-manager-cli` pelos SDKs Python/TypeScript. O gerenciador deve estar aberto na mesma sessão gráfica do usuário. No Windows, mantenha CLI e gerenciador instalados juntos; o bridge verifica seu servidor.

Antes de autorização, apenas capabilities e catálogo limitado estão disponíveis. A conexão humana define contas/grupos, ações e duração. A CLI conserva a chave de integração e faz prova nova por canal; não a exporta.

| Superfície | Métodos principais |
| --- | --- |
| Descoberta | `system/capabilities`, `catalog/read` |
| Conexão no bridge | `client/connect`, `client/authorize` |
| Metadados autorizados | `accounts/list`, `groups/list`, `bindings/list` |
| Execução efêmera | `execution/start`, `execution/status`, `execution/events`, `execution/cancel`, `execution/ack` |
| Harness App Server | `session/open`, `session/status`, `session/send`, `session/events`, `session/close`, `session/recover` |
| Gestão sob permissão | `groups/save`, `bindings/save`, `accounts/rename`, `accounts/setPolicy`, `accounts/archive`, `accounts/restore` |

Consulte os tipos do SDK e o dispatcher Rust para parâmetros e demais operações. Não infira disponibilidade a partir de um roadmap. Capabilities informa presença de código, não homologação completa de uma plataforma.

## Identidade e mudanças

Guarde `applicationId` da primeira resposta e use-o para reconectar. CLI/SDK alpha.8 retêm o ID durante a conexão atual. Use `authorize` para solicitar alteração de escopo no cadastro existente, não crie outro slug como workaround.

Sem ID, um slug exatamente existente pode encaminhar novo consentimento ao mesmo cadastro. Isso não recupera chave por nome nem concede acesso automático. O nome declarado não é publisher verificado.

## Execução

`accountId` e `groupId` selecionam recursos autorizados; IDs são opacos e exatos. Sem seleção, o gerenciador escolhe uma conta elegível antes de gerar. `bindingId` existe para compatibilidade, não é uma configuração adicional obrigatória.

Na alpha.8, a sessão já salva da conta pode ser reutilizada sem um login separado para aplicações. Metadados de sessão não comprovam autenticação remota. Política de uso, escopo, grupo, capacidade, pausa e revogação continuam aplicáveis.

`quota_exhausted`, `credential_unavailable`, `consent_required` e `authentication_required` têm significados distintos. Timeout de submissão não prova que o trabalho não iniciou. Preserve o ID, consulte o estado e não repita uma tarefa nem troque de conta automaticamente.

Frames/eventos são limitados. Histórico App Server é opt-in e privado. Consumidores não recebem autoridade para importar chats do Desktop por caminhos externos ou aprovar suas próprias ferramentas por conveniência.
