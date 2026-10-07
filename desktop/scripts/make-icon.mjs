// Generates desktop/app-icon.png (1024x1024 RGBA) with no image libraries.
//
// The icon is a rounded slate-blue tile with a bold white check mark, which is
// readable at 32 px. Run `npm run icon` to regenerate the Tauri icon set.

import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { encodePng } from "./png.mjs";

const SIZE = 1024;

/** @param {number} x @param {number} y */
function roundedTile(x, y) {
  const margin = 96;
  const radius = 200;
  const min = margin;
  const max = SIZE - margin;
  if (x < min || x > max || y < min || y > max) return false;
  const cx = Math.min(Math.max(x, min + radius), max - radius);
  const cy = Math.min(Math.max(y, min + radius), max - radius);
  const dx = x - cx;
  const dy = y - cy;
  return dx * dx + dy * dy <= radius * radius;
}

/** Distance from (x, y) to the segment (x1,y1)-(x2,y2). */
function distanceToSegment(x, y, x1, y1, x2, y2) {
  const vx = x2 - x1;
  const vy = y2 - y1;
  const wx = x - x1;
  const wy = y - y1;
  const t = Math.max(0, Math.min(1, (wx * vx + wy * vy) / (vx * vx + vy * vy)));
  const px = x1 + t * vx;
  const py = y1 + t * vy;
  return Math.hypot(x - px, y - py);
}

function pixel(x, y) {
  if (!roundedTile(x, y)) return [0, 0, 0, 0];
  const shade = 0.85 + 0.15 * (1 - y / SIZE);
  const base = [36, 74, 128, 255];
  const check =
    distanceToSegment(x, y, 330, 520, 460, 660) <= 52 ||
    distanceToSegment(x, y, 460, 660, 710, 380) <= 52;
  if (check) return [247, 250, 252, 255];
  return [
    Math.round(base[0] * shade),
    Math.round(base[1] * shade),
    Math.round(base[2] * shade),
    255,
  ];
}

const raw = Buffer.alloc(SIZE * (SIZE * 4 + 1));
let offset = 0;
for (let y = 0; y < SIZE; y += 1) {
  raw[offset] = 0;
  offset += 1;
  for (let x = 0; x < SIZE; x += 1) {
    const [r, g, b, a] = pixel(x, y);
    raw[offset] = r;
    raw[offset + 1] = g;
    raw[offset + 2] = b;
    raw[offset + 3] = a;
    offset += 4;
  }
}

const png = encodePng(SIZE, raw);

const here = dirname(fileURLToPath(import.meta.url));
const target = join(here, "..", "app-icon.png");
writeFileSync(target, png);
console.log(`wrote ${target} (${png.length} bytes)`);
