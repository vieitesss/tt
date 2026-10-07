// Generates the menu bar template icons with no image libraries.
//
// Pure-black glyphs on transparency: macOS template icons use the alpha
// channel only, so black is the canonical source color. Two files are written,
// both at 44 px (`tray-icon@2x.png`, `tray-icon-attention@2x.png`) — tray-icon
// scales whatever it is handed to 18 pt tall, so the larger source is the one
// that lands on the Retina grid. Run `npm run tray-icons`.

import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { encodePng } from "./png.mjs";

/** Logical glyph box, in points. Everything below is in these units. */
const UNIT = 22;
/** Rendered size: 2× the logical box, for the status bar's Retina grid. */
const SIZE = UNIT * 2;

/** Check mark stroke: polyline through these points, round caps. */
const CHECK = [
  [4.6, 12.0],
  [9.0, 16.3],
  [16.2, 6.5],
];
const CHECK_HALF_WIDTH = 1.15;
/** Attention dot. */
const DOT = { x: 18.8, y: 3.4, radius: 2.2 };

/** @param {number} x @param {number} y @param {number} x1 @param {number} y1 @param {number} x2 @param {number} y2 */
function distanceToSegment(x, y, x1, y1, x2, y2) {
  const vx = x2 - x1;
  const vy = y2 - y1;
  const wx = x - x1;
  const wy = y - y1;
  const t = Math.max(0, Math.min(1, (wx * vx + wy * vy) / (vx * vx + vy * vy)));
  return Math.hypot(x - (x1 + t * vx), y - (y1 + t * vy));
}

/** Whether a point is inside the glyph. @param {boolean} withDot */
function inside(x, y, withDot) {
  for (let i = 0; i + 1 < CHECK.length; i += 1) {
    const [x1, y1] = CHECK[i];
    const [x2, y2] = CHECK[i + 1];
    if (distanceToSegment(x, y, x1, y1, x2, y2) <= CHECK_HALF_WIDTH) return true;
  }
  if (withDot && Math.hypot(x - DOT.x, y - DOT.y) <= DOT.radius) return true;
  return false;
}

/** Supersampling factor per axis, for antialiasing. */
const SAMPLES = 8;

/** RGBA pixels for one size. @param {number} size @param {boolean} withDot */
function render(size, withDot) {
  const scale = size / UNIT;
  const raw = Buffer.alloc(size * (size * 4 + 1));
  let offset = 0;
  for (let py = 0; py < size; py += 1) {
    raw[offset] = 0; // PNG filter: none
    offset += 1;
    for (let px = 0; px < size; px += 1) {
      let hits = 0;
      for (let sy = 0; sy < SAMPLES; sy += 1) {
        for (let sx = 0; sx < SAMPLES; sx += 1) {
          const x = (px + (sx + 0.5) / SAMPLES) / scale;
          const y = (py + (sy + 0.5) / SAMPLES) / scale;
          if (inside(x, y, withDot)) hits += 1;
        }
      }
      raw[offset] = 0;
      raw[offset + 1] = 0;
      raw[offset + 2] = 0;
      raw[offset + 3] = Math.round((255 * hits) / (SAMPLES * SAMPLES));
      offset += 4;
    }
  }
  return raw;
}

const here = dirname(fileURLToPath(import.meta.url));
const icons = join(here, "..", "src-tauri", "icons");
for (const [name, withDot] of [
  ["tray-icon@2x", false],
  ["tray-icon-attention@2x", true],
]) {
  const file = join(icons, `${name}.png`);
  const bytes = encodePng(SIZE, render(SIZE, withDot));
  writeFileSync(file, bytes);
  console.log(`wrote ${file} (${SIZE}px, ${bytes.length} bytes)`);
}
