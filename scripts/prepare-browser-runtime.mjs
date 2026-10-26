import { createHash } from 'node:crypto';
import { chmodSync, createWriteStream, existsSync, mkdirSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { pipeline } from 'node:stream/promises';
import { Readable } from 'node:stream';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const manifestPath = resolve(projectRoot, 'src-tauri/resources/chromium-runtime-payload.json');
const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
const requestedPlatform = process.argv.find((argument) => argument.startsWith('--platform='))?.split('=')[1];
const platform = requestedPlatform ?? hostPlatform();
const artifact = manifest.artifacts.find((candidate) => candidate.platform === platform);

if (!artifact) throw new Error(`No pinned Chromium artifact for ${platform}`);

const cacheDirectory = resolve(projectRoot, '.runtime-cache');
const archivePath = resolve(cacheDirectory, `${manifest.version}-${platform}.zip`);
const runtimeParent = resolve(projectRoot, 'src-tauri/resources/runtime');
const runtimeDirectory = resolve(runtimeParent, 'chromium');
const stagingDirectory = resolve(runtimeParent, `.chromium-staging-${process.pid}`);
const previousDirectory = resolve(runtimeParent, `.chromium-previous-${process.pid}`);

mkdirSync(cacheDirectory, { recursive: true });
mkdirSync(runtimeParent, { recursive: true });

if (!(await archiveIsValid(archivePath, artifact))) {
  await downloadPinnedArchive(artifact, archivePath);
}

rmSync(stagingDirectory, { recursive: true, force: true });
rmSync(previousDirectory, { recursive: true, force: true });
mkdirSync(stagingDirectory, { recursive: true });
extractArchive(archivePath, stagingDirectory);

const executablePath = resolve(stagingDirectory, artifact.executable);
if (!existsSync(executablePath)) {
  throw new Error(`Chromium archive does not contain ${artifact.executable}`);
}
if (platform.startsWith('linux-')) makeLinuxLaunchersExecutable(stagingDirectory, artifact.executable);

writeFileSync(
  resolve(stagingDirectory, 'active.json'),
  `${JSON.stringify({
    version: manifest.version,
    executable: artifact.executable,
    archiveSha256: artifact.sha256,
  })}\n`,
  { mode: 0o644 },
);

if (existsSync(runtimeDirectory)) renameSync(runtimeDirectory, previousDirectory);
try {
  renameSync(stagingDirectory, runtimeDirectory);
} catch (error) {
  if (existsSync(previousDirectory)) renameSync(previousDirectory, runtimeDirectory);
  throw error;
}
rmSync(previousDirectory, { recursive: true, force: true });
console.log(`Prepared bundled Chromium ${manifest.version} for ${platform}.`);

function hostPlatform() {
  const os = process.platform === 'win32' ? 'windows' : process.platform;
  const architecture = process.arch === 'x64' ? 'x86_64' : process.arch;
  return `${os}-${architecture}`;
}

async function archiveIsValid(path, expected) {
  if (!existsSync(path) || statSync(path).size !== expected.size) return false;
  return (await sha256(path)) === expected.sha256;
}

async function downloadPinnedArchive(expected, destination) {
  const temporary = `${destination}.download-${process.pid}`;
  rmSync(temporary, { force: true });
  try {
    const response = await fetch(expected.url, { redirect: 'follow' });
    if (!response.ok || !response.body) {
      throw new Error(`Chromium download failed with HTTP ${response.status}`);
    }
    await pipeline(Readable.fromWeb(response.body), createWriteStream(temporary, { flags: 'wx' }));
    if (!(await archiveIsValid(temporary, expected))) {
      throw new Error('Downloaded Chromium did not match the pinned size and SHA-256 hash');
    }
  } catch (error) {
    rmSync(temporary, { force: true });
    throw error;
  }
  rmSync(destination, { force: true });
  renameSync(temporary, destination);
}

async function sha256(path) {
  const hash = createHash('sha256');
  const { createReadStream } = await import('node:fs');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('hex');
}

function extractArchive(archive, destination) {
  const command = process.platform === 'win32' ? 'tar.exe' : 'unzip';
  const arguments_ = process.platform === 'win32'
    ? ['-xf', archive, '-C', destination]
    : ['-q', archive, '-d', destination];
  const result = spawnSync(command, arguments_, { encoding: 'utf8' });
  if (result.status !== 0) {
    throw new Error(`Unable to extract Chromium: ${(result.stderr || result.stdout).trim()}`);
  }
}

function makeLinuxLaunchersExecutable(root, executable) {
  const directory = dirname(resolve(root, executable));
  for (const name of ['chrome', 'chrome-wrapper', 'chrome_crashpad_handler', 'chrome_sandbox', 'nacl_helper']) {
    const path = resolve(directory, name);
    if (existsSync(path) && statSync(path).isFile()) chmodSync(path, 0o755);
  }
}
