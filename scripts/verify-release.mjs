import { createHash, createPublicKey, timingSafeEqual, verify } from 'node:crypto';
import { createReadStream, readFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const [artifact, expectedVersion] = process.argv.slice(2);
if (!artifact || !expectedVersion)
  throw new Error('Usage: node scripts/verify-release.mjs ARTIFACT APP_VERSION');
const project = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const config = JSON.parse(readFileSync(resolve(project, 'src-tauri/tauri.conf.json'), 'utf8'));
const publicLines = Buffer.from(config.plugins.updater.pubkey, 'base64')
  .toString('utf8')
  .trim()
  .split(/\r?\n/);
const publicPacket = Buffer.from(publicLines[1] ?? '', 'base64');
const signatureText = readFileSync(`${artifact}.sig`, 'utf8').trim();
const lines = (
  signatureText.startsWith('untrusted comment:')
    ? signatureText
    : Buffer.from(signatureText, 'base64').toString('utf8')
)
  .trim()
  .split(/\r?\n/);
const signature = Buffer.from(lines[1] ?? '', 'base64');
const globalSignature = Buffer.from(lines[3] ?? '', 'base64');
if (
  publicPacket.length !== 42 ||
  signature.length !== 74 ||
  globalSignature.length !== 64 ||
  signature.subarray(0, 2).toString() !== 'ED'
)
  throw new Error('Unsupported or malformed signature');
if (!timingSafeEqual(publicPacket.subarray(2, 10), signature.subarray(2, 10)))
  throw new Error('Signing key identity mismatch');
if (!lines[2]?.startsWith('trusted comment: '))
  throw new Error('Missing authenticated release metadata');
const trusted = lines[2].slice('trusted comment: '.length);
if (!trusted.split('\t').includes(`version:${expectedVersion}`))
  throw new Error('Authenticated app version mismatch');
const publicKey = createPublicKey({
  key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), publicPacket.subarray(10)]),
  format: 'der',
  type: 'spki',
});
const digest = createHash('blake2b512');
for await (const chunk of createReadStream(artifact)) digest.update(chunk);
if (!verify(null, digest.digest(), publicKey, signature.subarray(10)))
  throw new Error('Artifact signature invalid');
if (
  !verify(
    null,
    Buffer.concat([signature.subarray(10), Buffer.from(trusted)]),
    publicKey,
    globalSignature,
  )
)
  throw new Error('Release metadata signature invalid');
console.log(`Verified artifact and authenticated version ${expectedVersion}: ${artifact}`);
