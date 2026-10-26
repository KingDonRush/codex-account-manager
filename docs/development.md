# Desenvolvimento

## Requisitos

- Node.js 22 ou mais recente e npm.
- Rust estável compatível com o lockfile; respeite `rust-version` em `src-tauri/Cargo.toml`.
- Dependências nativas do Tauri 2: WebKitGTK 4.1/GTK3 no Linux; MSVC Build Tools e WebView2 no Windows.
- Uma instalação autorizada do Codex para operações reais. Testes sintéticos não precisam autenticar uma conta.

Consulte os [pré-requisitos oficiais do Tauri](https://v2.tauri.app/start/prerequisites/) para a sua distribuição. No Linux, o suporte de bandeja também precisa de AppIndicator/Ayatana conforme o sistema.

## Interface com dados sintéticos

```sh
npm ci
VITE_USE_MOCK_API=true npm run dev
```

No PowerShell, configure `$env:VITE_USE_MOCK_API='true'` antes de executar `npm run dev`. Esse modo não comprova acesso real ao cofre, ao Codex ou ao navegador.

## Aplicativo nativo e runtime

```sh
npm run runtime:prepare
npm run tauri dev
```

O preparador baixa o runtime fixado para a plataforma e verifica tamanho e hash. O manifesto do runtime tem verificação própria no backend. Não substitua o runtime por um download de origem desconhecida.

## Verificações

```sh
npm run check
npm run test:unit
npm run sdk:build
python3 -B -m unittest discover -s integration/python -p test_client.py
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml --lib
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
```

Para os testes de navegador da interface, instale o Chromium do Playwright e execute `npm run test:e2e` e `npm run test:usability`. Os testes nativos marcados `ignored` são opt-in e descrevem seus requisitos; não execute todos indiscriminadamente contra contas ou perfis reais.

Antes das verificações Rust numa cópia recém-clonada, execute `npm run sdk:build`
e `npm run runtime:prepare`: o gerador Tauri confere a presença dos recursos do
pacote mesmo quando se compila somente a biblioteca de testes.

## Empacotar

```sh
npm run sdk:build
npm run runtime:prepare
cargo build --manifest-path src-tauri/Cargo.toml --release --bin codex-account-manager-cli --features custom-protocol --locked
```

Linux: `npm run tauri build -- --bundles deb`. Windows: `npm run tauri build -- --config src-tauri/tauri.bundle-windows.conf.json --bundles nsis`.

Para uma publicação, compile em ambiente neutro ou use `--remap-path-prefix` do Rust. Binários também podem conter caminhos locais de compilação: verificar apenas `.gitignore` é insuficiente. Não publique chaves privadas nem configure um endpoint de atualização sem administrar sua assinatura e compatibilidade.

## Conteúdo deste snapshot

Este é um snapshot público revisado, não uma cópia da pasta privada de desenvolvimento. Instruções e memórias pessoais de agentes, relatórios de incidentes individuais, bancos, perfis e uma ferramenta manual específica de reparo de instalação foram excluídos. O reparo manual não faz parte do aplicativo distribuído. Testes sintéticos e mecanismos de migração do produto foram mantidos.
