import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import { chromium } from 'playwright';

const [src, out] = process.argv.slice(2);
if (!src || !out) {
  console.error('usage: node svg2png.mjs <in.svg> <out.png>');
  process.exit(2);
}
const svg = readFileSync(src, 'utf8');
const vb = svg.match(/viewBox="([\d.\s-]+)"/)?.[1]?.trim().split(/\s+/).map(Number);
const wAttr = Number(svg.match(/<svg[^>]*\swidth="([\d.]+)"/)?.[1] ?? NaN);
const hAttr = Number(svg.match(/<svg[^>]*\sheight="([\d.]+)"/)?.[1] ?? NaN);
const w = Math.ceil(wAttr || (vb ? vb[2] : 800));
const h = Math.ceil(hAttr || (vb ? vb[3] : 600));

const browser = await chromium.launch();
const page = await browser.newPage({
  viewport: { width: w, height: h },
  deviceScaleFactor: 2,
});
await page.goto(pathToFileURL(src).href);
await page.locator('svg').screenshot({ path: out });
await browser.close();
console.log(`${out}: ${w}x${h} @2x`);
