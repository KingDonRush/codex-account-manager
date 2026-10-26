# Segurança e relatos responsáveis

Este gerenciador manipula autenticação local. Use uma versão compatível, baixe apenas de fontes que você verificou e confira os checksums disponibilizados com cada release.

## Não publique dados privados

Não inclua tokens, cookies, senhas, chaves, bancos de dados, perfis de navegador, conteúdo de chats, URLs completas de OAuth ou diagnósticos brutos em issues e pull requests. Reproduções devem usar contas e exemplos sintéticos.

Se encontrar uma vulnerabilidade que permita leitura de credenciais ou execução não autorizada, use o recurso **Report a vulnerability** na aba Security quando estiver disponível. Não publique a prova com segredos. Na ausência de um canal privado, abra apenas um aviso sem detalhes exploráveis e combine o canal de relato com o mantenedor.

## Limites

- O projeto está em alpha, sem garantia de segurança absoluta.
- O cofre do sistema operacional e as permissões da sessão fazem parte da fronteira de confiança.
- Não há isolamento contra root, administrador, malware ou ferramentas de depuração do mesmo usuário.
- Autorizações de integração não equivalem a autenticação de publisher.
- Nunca use dados reais para testar políticas de exclusão, renovação ou isolamento.
