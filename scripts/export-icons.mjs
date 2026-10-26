import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const catalogPath = resolve(root, 'src/lib/icon-catalog.json');
const catalog = JSON.parse(readFileSync(catalogPath, 'utf8'));
const svgDirectory = resolve(root, 'design/icons-svg');
const pngDirectory = resolve(root, 'design/icons-png');

mkdirSync(svgDirectory, { recursive: true });
mkdirSync(pngDirectory, { recursive: true });

const svgDocument = (markup, title = '') => `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="64" height="64" fill="none" color="#4f46e5" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
  ${title ? `<title>${escapeHtml(title)}</title>` : ''}${markup}
</svg>\n`;

for (const [name, icon] of Object.entries(catalog)) {
  writeFileSync(resolve(svgDirectory, `${name}.svg`), svgDocument(icon.markup, icon.label));
}

const cards = Object.entries(catalog).map(([name, icon]) => `
  <figure class="icon-card" data-icon="${name}">
    ${svgDocument(icon.markup).replace('<?xml version="1.0" encoding="UTF-8"?>', '')}
    <figcaption>${escapeHtml(icon.label)}</figcaption>
  </figure>`).join('');
const html = `<!doctype html><html><head><meta charset="utf-8"><style>
  :root { color-scheme: light; font-family: Inter, system-ui, sans-serif; }
  html, body { margin: 0; background: transparent; }
  #atlas { display: grid; grid-template-columns: repeat(7, 112px); gap: 16px; padding: 24px; width: max-content; }
  .icon-card { margin: 0; width: 112px; height: 108px; display: grid; place-items: center; align-content: center; gap: 8px; color: #172033; }
  .icon-card svg { width: 58px; height: 58px; color: #4f46e5; stroke: currentColor; }
  figcaption { width: 100%; overflow: hidden; text-align: center; white-space: nowrap; text-overflow: ellipsis; font-size: 12px; font-weight: 650; }
</style></head><body><main id="atlas">${cards}</main></body></html>`;

writeFileSync(resolve(root, 'design/icon-catalog.html'), html);
writeFileSync(resolve(root, 'design/icon-catalog.md'), markdownCatalog(catalog));

const browser = await chromium.launch({
  executablePath: '/usr/bin/google-chrome',
});
const page = await browser.newPage({ viewport: { width: 960, height: 760 }, deviceScaleFactor: 2 });
await page.setContent(html);
await page.locator('#atlas').screenshot({ path: resolve(root, 'design/icon-catalog.png'), omitBackground: true });

const dimensions = {};
try {
  for (const name of Object.keys(catalog)) {
    const buffer = await page.locator(`[data-icon="${name}"] svg`).screenshot({ omitBackground: true });
    const cropped = await trimAlpha(page, buffer);
    writeFileSync(resolve(pngDirectory, `${name}.png`), Buffer.from(cropped.png, 'base64'));
    dimensions[name] = { width: cropped.width, height: cropped.height };
  }
  writeFileSync(resolve(root, 'design/icon-dimensions.json'), JSON.stringify(dimensions, null, 2) + '\n');
  for (const theme of ['light', 'dark']) {
    const board = assetBoard(theme);
    writeFileSync(resolve(root, `design/assets-${theme}.html`), board);
    await page.setContent(board);
    await page.locator('.asset-board').screenshot({ path: resolve(root, `design/assets-${theme}.png`) });
  }
} finally { await browser.close(); }
console.log(`Exported ${Object.keys(catalog).length} alpha-trimmed icons and light/dark asset boards.`);

async function trimAlpha(page, buffer) {
  return page.evaluate(async (url) => {
    const image = new Image(); image.src = url; await image.decode();
    const canvas = document.createElement('canvas'); canvas.width = image.width; canvas.height = image.height;
    const context = canvas.getContext('2d'); context.drawImage(image, 0, 0);
    const pixels = context.getImageData(0, 0, canvas.width, canvas.height).data;
    let left = canvas.width, top = canvas.height, right = -1, bottom = -1;
    for (let y = 0; y < canvas.height; y++) for (let x = 0; x < canvas.width; x++) {
      if (!pixels[(y * canvas.width + x) * 4 + 3]) continue;
      left = Math.min(left, x); top = Math.min(top, y); right = Math.max(right, x); bottom = Math.max(bottom, y);
    }
    if (right < 0) throw new Error('Empty icon');
    const crop = document.createElement('canvas'); crop.width = right - left + 1; crop.height = bottom - top + 1;
    crop.getContext('2d').drawImage(canvas, left, top, crop.width, crop.height, 0, 0, crop.width, crop.height);
    return { png: crop.toDataURL('image/png').split(',')[1], width: crop.width, height: crop.height };
  }, `data:image/png;base64,${buffer.toString('base64')}`);
}

function assetBoard(theme) {
  const dark = theme === 'dark';
  const groups = Map.groupBy(Object.entries(catalog), ([, icon]) => icon.group);
  const sections = [...groups].map(([group, entries]) => `<section><h2>${escapeHtml(group)}</h2><div class="group">${entries.map(([name, icon]) => `<figure>${svgDocument(icon.markup).replace('<?xml version="1.0" encoding="UTF-8"?>', '')}<figcaption>${escapeHtml(icon.label)}</figcaption><small>${name}</small></figure>`).join('')}</div></section>`).join('');
  const illustrations = ['accounts-empty', 'session-recovery'].map((name) => `<img src="data:image/png;base64,${readFileSync(resolve(root, `src/assets/${name}.png`)).toString('base64')}" alt="${name}" />`).join('');
  return `<!doctype html><html lang="pt-BR"><meta charset="utf-8"><style>
    *{box-sizing:border-box}body{margin:0;font-family:system-ui,sans-serif;color:${dark ? '#e3e6ef' : '#202737'};background:${dark ? '#14161d' : '#f7f8fa'}}
    .asset-board{width:1010px;padding:42px}header{display:flex;justify-content:space-between;align-items:center;border-bottom:1px solid ${dark ? '#343949' : '#e3e7ef'};padding-bottom:22px;margin-bottom:28px}
    h1{font-size:26px;font-weight:600;letter-spacing:-.7px;margin:0}header p,h2,small{color:${dark ? '#a3acc0' : '#647087'}}header p{font-size:12px;margin:6px 0 0}.illustrations{display:flex;gap:15px}.illustrations img{width:70px;height:70px;object-fit:contain}
    h2{font-size:11px;font-weight:500;letter-spacing:1px;text-transform:uppercase;margin:22px 0 9px}.group{display:grid;grid-template-columns:repeat(7,1fr);gap:8px}
    figure{margin:0;display:grid;justify-items:center;align-content:center;height:92px;gap:5px;border-radius:8px;background:${dark ? '#1c1f28' : '#ffffff'}}svg{width:26px;height:26px;color:${dark ? '#aca4ff' : '#6259de'};stroke:currentColor}figcaption{font-size:10px;max-width:126px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}small{font-size:8px}
    footer{display:flex;gap:18px;margin-top:28px;font-size:10px;color:${dark ? '#a3acc0' : '#647087'}}
  </style><main class="asset-board"><header><div><h1>Codex · assets 0.4</h1><p>Traço consistente. Ações pequenas. Informação sob demanda.</p></div><div class="illustrations">${illustrations}</div></header>${sections}<footer><span>${Object.keys(catalog).length} ícones SVG + PNG transparente</span><span>Trim alpha individual</span><span>Tema ${dark ? 'escuro' : 'claro'}</span></footer></main></html>`;
}

function markdownCatalog(items) {
  const groups = Map.groupBy(Object.entries(items), ([, icon]) => icon.group);
  const sections = [...groups.entries()].map(([group, entries]) => {
    const rows = entries.map(([name, icon]) => `| \`${name}\` | ${icon.label} |`).join('\n');
    return `## ${group}\n\n| Asset | Uso |\n|---|---|\n${rows}`;
  });
  return `# Catálogo de ícones\n\nFonte única usada pela interface e pelos exports SVG/PNG. Todos os arquivos individuais têm fundo transparente.\n\n${sections.join('\n\n')}\n`;
}

function escapeHtml(value) {
  return value.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;');
}
