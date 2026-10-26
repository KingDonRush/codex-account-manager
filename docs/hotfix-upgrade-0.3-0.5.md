# Atualizar preservando contas

O identificador do aplicativo, o serviço do cofre e o diretório de dados são mantidos entre as versões Rust suportadas. O código possui migrações transacionais e reconciliação de metadados desde a série 0.3. Isso não é importação do antigo aplicativo Python nem portabilidade de credenciais entre usuários ou sistemas operacionais.

1. Encerre trabalhos das aplicações e operações de conexão antes da atualização.
2. Saia do gerenciador pelo comando de saída completa, não apenas ocultando sua janela.
3. Instale por cima no mesmo usuário e destino. Não desinstale apagando os dados.
4. Ao abrir, confira as contas, associações e autorizações antes de iniciar trabalho.

O cofre depende da sessão do sistema operacional. Erros ao acessá-lo precisam aparecer como indisponibilidade ou recuperação pendente, não como conta inexistente. O aplicativo não deve mandar repetir OAuth simplesmente porque uma consulta não retornou identidade.

Em Diagnóstico, a ação de recuperar contas salvas se limita ao armazenamento do próprio gerenciador. Arquivamento e exclusão explícita devem ser respeitados.

Não copie `auth.json`, cookies, perfis de navegador ou bancos para outro computador como substituto de um fluxo autorizado de autenticação. Não use refresh de gerações antigas como fallback automático. Um binário de versão anterior também não prova que formatos e credenciais atuais podem sofrer downgrade com segurança.

Versões alpha podem exigir reparos e possuem limites documentados. Relate falhas com códigos e dados sintéticos; não publique seu banco, cofre ou diagnóstico bruto.
