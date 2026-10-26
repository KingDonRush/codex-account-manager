// Keep the signed runtime payload byte-identical across Linux/Windows checkouts.
import { readFileSync } from 'node:fs';
import { createPublicKey, verify } from 'node:crypto';
import { fileURLToPath } from 'node:url';
const root = new URL('../', import.meta.url);
const source = readFileSync(new URL('src-tauri/src/adapters/runtime_installer.rs', root), 'utf8');
const hex = source.match(/const MANIFEST_PUBLIC_KEY: &str\s*=\s*"([a-f0-9]{64})"/)?.[1];
if (!hex) throw new Error('Canonical runtime verification key not found');
const key = createPublicKey({
  key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), Buffer.from(hex, 'hex')]),
  format: 'der', type: 'spki',
});
const payload = readFileSync(new URL('src-tauri/resources/chromium-runtime-payload.json', root));
const signature = Buffer.from(readFileSync(new URL('src-tauri/resources/chromium-runtime.sig', root), 'utf8').trim(), 'hex');
if (signature.length !== 64 || !verify(null, payload, key, signature))
  throw new Error('Runtime manifest signature mismatch; preserve original LF line endings');
console.log(`Signed runtime manifest verified: ${fileURLToPath(new URL('src-tauri/resources/chromium-runtime-payload.json', root)).split('/').pop()}`);
