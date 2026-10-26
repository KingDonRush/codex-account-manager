# Aplicações e harnesses

Instale o cliente Python a partir de `integration/python` no ambiente do seu projeto. O cliente TypeScript está em `integration/typescript`; compile com `npm run sdk:build`.

No Linux, a CLI empacotada é `codex-account-manager-cli`. No Windows, informe o caminho da CLI instalada ao lado do gerenciador se ela não estiver no PATH. O gerenciador deve estar aberto na mesma sessão gráfica.

## Conexão e alteração

```python
from codex_account_manager import Client

with Client() as manager:
    receipt = manager.connect("Minha ferramenta", "minha-ferramenta")
    # Aguarda aprovação humana no gerenciador.
    application_id = receipt["applicationId"]  # Persistir no consumidor.
    accounts = manager.request("accounts/list")
```

A aprovação já escolhe recursos e uso compartilhado. Não peça um vínculo depois nem outro login só para aplicações se a conta já estiver salva. Para ampliar acesso, chame `manager.authorize(...)` no mesmo cadastro, com o novo escopo mínimo. Após reiniciar o consumidor, passe `application_id` explicitamente a `connect` para reutilizar uma autorização durável aprovada.

## Execução efêmera

```python
from pathlib import Path
from codex_account_manager import Client

with Client() as manager:
    manager.connect("Minha ferramenta", "minha-ferramenta",
                    application_id=saved_application_id)
    job = manager.execute(prompt, cwd=Path.cwd(),
                          arguments=["--sandbox", "read-only"],
                          account_id=selected_account_id)
    for event in manager.events(job["id"]):
        consume_private_event(event)
```

As variáveis de aplicação no exemplo devem vir do seu projeto e de IDs autorizados, não de busca por nome/e-mail. `read-only` é a escolha deste exemplo, não uma permissão imposta a todos os consumidores.

Depois de aceito um trabalho, conserve seu ID. Não repita inferência após timeout: ela pode ter começado. Revise estado e saída antes de decidir outra ação.

## App Server

```python
with Client() as manager:
    manager.connect("Minha ferramenta", "minha-ferramenta",
                    application_id=saved_application_id)
    with manager.open_session(cwd=workspace, account_id=selected_account_id) as session:
        thread = session.rpc("thread/start", {
            "cwd": str(workspace), "sandbox": "readOnly", "approvalPolicy": "never",
        })
        result = session.rpc("turn/start", {
            "threadId": thread["thread"]["id"],
            "input": [{"type": "text", "text": prompt}],
        }, host_handler=handle_host_request)
```

Seu harness deve implementar `handle_host_request` com validação e aprovação proporcionais. Não configure aceitação automática de tudo. A ausência de handler não autoriza o gerenciador a decidir por você.

Use um leitor de eventos por sessão e encerre cliente/sessão em `finally` ou context manager. O histórico é opt-in. `codex exec` com acesso externo não tem callback de refresh durante o job; prefira App Server para operações duradouras.

No TypeScript, use `Client.connect`, `Client.execute` e `Client.openSession`; a sessão oferece `rpc`, `events`, `respond` e `close`. Clientes sync/async Python preservam o mesmo contrato. Exemplos e testes não autorizam ler credenciais diretamente.
