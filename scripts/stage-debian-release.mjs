// Package-generated metadata only; never opens or modifies a user's app data.
import { readFileSync, writeFileSync, mkdirSync, existsSync, cpSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const config = JSON.parse(readFileSync(resolve(root, 'src-tauri/tauri.conf.json'), 'utf8'));
const version = config.version;
if (!/^\d+\.\d+\.\d+(?:-[\w.]+)?$/.test(version)) throw new Error('Unsupported app version');
const debVersion = version.replace('-', '~');
const staging = resolve(
  root,
  `src-tauri/target/release/bundle/deb/${config.productName}_${version}_amd64`,
);
const control = resolve(staging, 'control/control');
const metadata = readFileSync(control, 'utf8');
if (
  !metadata.includes('Package: gerenciador-de-contas-do-codex\n') ||
  !metadata.includes('Architecture: amd64\n')
)
  throw new Error('Unexpected package identity/architecture');
const normalized = metadata.replace(/^Version: .+$/m, `Version: ${debVersion}`);
const out = resolve(root, `release/${version}/linux`);
const artifact = resolve(out, `codex-account-manager_${debVersion}_amd64.deb`);
if (existsSync(artifact)) throw new Error(`Release artifact already exists: ${artifact}`);
mkdirSync(out, { recursive: true });
// '~' sorts an alpha before the future stable version in Debian's ordering.
writeFileSync(control, normalized);
cpSync(resolve(staging, 'control'), resolve(staging, 'data/DEBIAN'), { recursive: true });
execFileSync('dpkg-deb', ['--root-owner-group', '-Zxz', '-b', resolve(staging, 'data'), artifact], {
  stdio: 'inherit',
});
const actual = execFileSync('dpkg-deb', ['-f', artifact, 'Version'], { encoding: 'utf8' }).trim();
if (actual !== debVersion) throw new Error('Debian version mismatch');
console.log(artifact);
