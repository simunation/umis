// 2D canvas view of a Game of Life lattice (`life2d` zones): every zone's
// board stitched at its global origin, zone borders coloured by commitment
// status, per-zone generation labels, trails (cells alive k generations ago),
// and ghost tiles at free lattice points (click to propose a zone).

import { RenderGrid } from './api';

export interface ZoneTile {
  zone: number;
  point: [number, number, number];
  grid: RenderGrid | null;
  /** Latest commitment status: optimistic / proven / settled / invalid. */
  status: string | null;
  /** Beacon slot of the zone's last rollback, if recent. */
  rolledBack: boolean;
}

interface Meta { born: boolean; gen: number; birth: number; next_slot: number; k: number; population: number }

const css = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

export class View2d {
  private canvas = document.createElement('canvas');
  private ctx: CanvasRenderingContext2D;
  private tiles: ZoneTile[] = [];
  private ghosts: [number, number, number][] = [];
  private n = 64;
  private scale = 4;
  private pan = { x: 24, y: 24 };
  private drag: { x: number; y: number; px: number; py: number; moved: boolean } | null = null;
  private hover: [number, number] | null = null;
  private fitted = false;
  onGhostClick: (p: [number, number, number]) => void = () => {};
  onZoneClick: (zone: number) => void = () => {};
  showTrails = true;
  showLabels = true;

  constructor(private el: HTMLElement) {
    this.ctx = this.canvas.getContext('2d')!;
    el.appendChild(this.canvas);
    new ResizeObserver(() => this.resize()).observe(el);
    this.bind();
    this.resize();
  }

  set zoneSize(n: number) {
    this.n = n;
  }

  setTiles(tiles: ZoneTile[]) {
    this.tiles = tiles;
    if (!this.fitted && tiles.length) {
      this.fit();
      this.fitted = true;
    }
    this.draw();
  }

  setGhosts(points: [number, number, number][]) {
    this.ghosts = points.filter((p) => p[2] === 0);
    this.draw();
  }

  /** Fits occupied lattice points (plus one ring of ghosts) into the canvas. */
  fit() {
    const pts = [...this.tiles.map((t) => t.point), ...this.ghosts];
    if (!pts.length) return;
    const xs = pts.map((p) => p[0]);
    const ys = pts.map((p) => p[1]);
    const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs) + 1, Math.min(...ys), Math.max(...ys) + 1];
    const w = this.canvas.clientWidth;
    const h = this.canvas.clientHeight;
    const cells = Math.max((x1 - x0) * this.n, 1);
    const rows = Math.max((y1 - y0) * this.n, 1);
    this.scale = Math.max(0.5, Math.min((w - 48) / cells, (h - 48) / rows));
    this.pan = { x: (w - cells * this.scale) / 2 - x0 * this.n * this.scale, y: (h - rows * this.scale) / 2 + y1 * this.n * this.scale };
    this.draw();
  }

  private resize() {
    const r = this.el.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    this.canvas.width = Math.max(1, Math.floor(r.width * dpr));
    this.canvas.height = Math.max(1, Math.floor(r.height * dpr));
    this.canvas.style.width = `${r.width}px`;
    this.canvas.style.height = `${r.height}px`;
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    this.draw();
  }

  /** Screen position of global cell (x, y); y grows upward in the world. */
  private toScreen(x: number, y: number): [number, number] {
    return [this.pan.x + x * this.scale, this.pan.y - y * this.scale];
  }

  private toCell(sx: number, sy: number): [number, number] {
    return [Math.floor((sx - this.pan.x) / this.scale), Math.floor((this.pan.y - sy) / this.scale)];
  }

  private bind() {
    const c = this.canvas;
    c.addEventListener('pointerdown', (e) => {
      this.drag = { x: e.clientX, y: e.clientY, px: this.pan.x, py: this.pan.y, moved: false };
      c.setPointerCapture(e.pointerId);
    });
    c.addEventListener('pointermove', (e) => {
      const r = c.getBoundingClientRect();
      this.hover = this.toCell(e.clientX - r.left, e.clientY - r.top);
      if (this.drag) {
        const dx = e.clientX - this.drag.x;
        const dy = e.clientY - this.drag.y;
        if (Math.abs(dx) + Math.abs(dy) > 3) this.drag.moved = true;
        this.pan = { x: this.drag.px + dx, y: this.drag.py + dy };
      }
      this.draw();
    });
    c.addEventListener('pointerup', (e) => {
      const d = this.drag;
      this.drag = null;
      if (d && !d.moved) this.click(e);
    });
    c.addEventListener('pointerleave', () => {
      this.hover = null;
      this.draw();
    });
    c.addEventListener(
      'wheel',
      (e) => {
        e.preventDefault();
        const r = c.getBoundingClientRect();
        const [mx, my] = [e.clientX - r.left, e.clientY - r.top];
        const f = Math.exp(-e.deltaY * 0.0015);
        const s = Math.min(40, Math.max(0.25, this.scale * f));
        this.pan = { x: mx - ((mx - this.pan.x) * s) / this.scale, y: my - ((my - this.pan.y) * s) / this.scale };
        this.scale = s;
        this.draw();
      },
      { passive: false },
    );
    c.addEventListener('dblclick', () => this.fit());
  }

  private click(e: PointerEvent) {
    const r = this.canvas.getBoundingClientRect();
    const [cx, cy] = this.toCell(e.clientX - r.left, e.clientY - r.top);
    const p: [number, number] = [Math.floor(cx / this.n), Math.floor(cy / this.n)];
    const t = this.tiles.find((t) => t.point[0] === p[0] && t.point[1] === p[1]);
    if (t) return this.onZoneClick(t.zone);
    const g = this.ghosts.find((q) => q[0] === p[0] && q[1] === p[1]);
    if (g) this.onGhostClick(g);
  }

  draw() {
    const ctx = this.ctx;
    const w = this.canvas.clientWidth;
    const h = this.canvas.clientHeight;
    ctx.fillStyle = css('--life-bg') || '#0b0f17';
    ctx.fillRect(0, 0, w, h);
    const n = this.n;
    const s = this.scale;
    const maxGen = Math.max(0, ...this.tiles.map((t) => (t.grid?.meta as Meta | undefined)?.gen ?? 0));

    // ghosts
    ctx.setLineDash([4, 4]);
    ctx.lineWidth = 1;
    for (const g of this.ghosts) {
      const [sx, sy] = this.toScreen(g[0] * n, (g[1] + 1) * n);
      ctx.strokeStyle = css('--life-ghost') || '#3a4458';
      ctx.strokeRect(sx + 0.5, sy + 0.5, n * s - 1, n * s - 1);
      if (n * s > 40) {
        ctx.fillStyle = css('--life-ghost') || '#3a4458';
        ctx.font = `${Math.min(28, (n * s) / 4)}px system-ui`;
        ctx.textAlign = 'center';
        ctx.textBaseline = 'middle';
        ctx.fillText('+', sx + (n * s) / 2, sy + (n * s) / 2);
      }
    }
    ctx.setLineDash([]);

    // cells
    const alive = css('--life-cell') || '#e8f0ff';
    const trail = css('--life-trail') || '#2f4a7a';
    const unborn = css('--life-unborn') || '#151b26';
    for (const t of this.tiles) {
      const g = t.grid;
      const [ox, oy] = this.toScreen(t.point[0] * n, (t.point[1] + 1) * n);
      const meta = g?.meta as Meta | undefined;
      if (!g || !meta?.born) {
        ctx.fillStyle = unborn;
        ctx.fillRect(ox, oy, n * s, n * s);
      } else {
        const d = g.data;
        const px = Math.max(s, 1);
        for (let y = 0; y < n; y++) {
          for (let x = 0; x < n; x++) {
            const i = 2 * (y * n + x);
            const [sx, sy] = [ox + x * s, oy + (n - 1 - y) * s];
            if (d[i]) {
              ctx.fillStyle = alive;
              ctx.fillRect(sx, sy, px, px);
            } else if (this.showTrails && d[i + 1]) {
              ctx.fillStyle = trail;
              ctx.fillRect(sx, sy, px, px);
            }
          }
        }
      }
    }

    // borders and labels
    for (const t of this.tiles) {
      const [ox, oy] = this.toScreen(t.point[0] * n, (t.point[1] + 1) * n);
      const meta = t.grid?.meta as Meta | undefined;
      const col = css(`--${t.rolledBack ? 'invalid' : (t.status ?? 'line')}`) || '#888';
      ctx.strokeStyle = col;
      ctx.lineWidth = t.rolledBack ? 3 : 2;
      ctx.strokeRect(ox + 1, oy + 1, n * s - 2, n * s - 2);
      if (!this.showLabels || n * s < 70) continue;
      const lag = meta?.born ? maxGen - meta.gen : 0;
      const text = meta
        ? meta.born
          ? `z${t.zone} · gen ${meta.gen}${lag > 0 ? ` (−${lag})` : ''} · ${meta.population}`
          : `z${t.zone} · unborn`
        : `z${t.zone} · not followed`;
      ctx.save();
      ctx.beginPath();
      ctx.rect(ox, oy, n * s, n * s);
      ctx.clip();
      ctx.font = '11px ui-monospace, monospace';
      ctx.textAlign = 'left';
      ctx.textBaseline = 'top';
      const tw = ctx.measureText(text).width + 8;
      ctx.fillStyle = 'rgba(8,11,18,0.78)';
      ctx.fillRect(ox + 4, oy + 4, tw, 16);
      ctx.fillStyle = lag > 0 ? css('--optimistic') : '#dfe6f3';
      ctx.fillText(text, ox + 8, oy + 6);
      if (t.status) {
        const badge = t.rolledBack ? 'rollback' : t.status;
        ctx.font = '10px system-ui';
        const bw = ctx.measureText(badge).width + 10;
        ctx.fillStyle = col;
        ctx.fillRect(ox + n * s - bw - 4, oy + n * s - 19, bw, 15);
        ctx.fillStyle = '#10131a';
        ctx.fillText(badge, ox + n * s - bw + 1, oy + n * s - 17);
      }
      ctx.restore();
    }

    // hover readout
    if (this.hover) {
      const [cx, cy] = this.hover;
      const p = [Math.floor(cx / n), Math.floor(cy / n)];
      const t = this.tiles.find((t) => t.point[0] === p[0] && t.point[1] === p[1]);
      const ghost = !t && this.ghosts.some((g) => g[0] === p[0] && g[1] === p[1]);
      const label = t
        ? `cell (${cx}, ${cy}) · zone ${t.zone} at (${p[0]}, ${p[1]})`
        : ghost
          ? `free point (${p[0]}, ${p[1]}) · click to propose a zone`
          : `(${cx}, ${cy})`;
      ctx.font = '11px ui-monospace, monospace';
      ctx.textAlign = 'left';
      ctx.textBaseline = 'bottom';
      const tw = ctx.measureText(label).width + 10;
      ctx.fillStyle = 'rgba(8,11,18,0.85)';
      ctx.fillRect(8, h - 24, tw, 18);
      ctx.fillStyle = '#dfe6f3';
      ctx.fillText(label, 13, h - 9);
    }
  }
}
