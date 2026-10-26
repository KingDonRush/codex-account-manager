# Contrato de segurança — candidata 0.5

## Fronteiras

O gerenciador mantém metadados locais e usa o cofre nativo para credenciais. O frontend não recebe tokens OAuth. Integrações utilizam a CLI oficial do projeto em vez de ler SQLite, arquivos de autenticação ou cookies.

O transporte de integrações é local: socket Unix privado no Linux, Named Pipe por usuário/sessão no Windows. A identidade do processo é verificada pelo sistema operacional; isso não autentica o publisher do programa consumidor nem isola programas maliciosos sob o mesmo usuário.

## Consentimento

A primeira conexão e a ampliação de acesso requerem decisão humana no gerenciador. A pessoa confere o código e seleciona recursos, permissões e duração. Nome, slug e associação a grupo não concedem execução por si.

Uma autorização durável pode ser reutilizada com nova prova criptográfica por canal. A chave permanece no bridge/cofre, não é devolvida pelo SDK. Revogação e expiração recusam novos usos. Exclusão permanente exige confirmação própria mesmo com acesso administrativo autorizado.

## Autenticação e execução

Na alpha.8, uma conta salva pode atender aplicações sem outro OAuth exclusivo. O núcleo prepara uma fonte de acesso para o processo próprio, com identidade fixa, sem entregar refresh ao consumidor. Fontes inativas próximas de expirar são coordenadas sob o lock da troca e persistência protegida. A fonte ativa continua sob o dono Desktop: o gerenciador relê o acesso atual, sem renovar uma cópia concorrente.

Sessões dedicadas antigas continuam preservadas. O estado “sessão registrada” não comprova autenticação remota; a validação da execução pode falhar honestamente. Rede/cofre indisponível não equivale a revogação.

Exec externo não tem callback de renovação dentro do job. App Server é a opção para harnesses duradouros com pedidos de host. O consumidor define ferramentas, sandbox e aprovações; o gerenciador não aprova esses pedidos automaticamente.

## Preservação e limites

Troca de conta precisa preservar o estado mais recente antes do commit. Uma falha não autoriza replay de refresh antigo, descarte de cadastro, repetição de inferência ou troca de identidade no meio do turno. Atualizar o aplicativo não é motivo para apagar contas.

Runtimes têm dono e encerramento. Cancelamento de uma operação deve fechar somente seus alvos; uma aba manual não pode ser confundida com o login atual. O navegador normal usa extensão privada e Native Messaging, não scripts injetados em páginas de login.

Não há garantia contra malware, administrador, root ou depuração local. Windows elevado possui limitação conhecida de integração com o navegador; use o aplicativo sem elevação. Fechamento/reabertura do Codex e mudanças de versões externas ainda exigem validação adicional. Consulte [SECURITY.md](../SECURITY.md) antes de enviar relatos.
