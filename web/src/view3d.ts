// three.js view of the lattice: zone states stitched in global coordinates
// (velocity arrows + dye points), ghost blocks for free locations.

import * as THREE from 'three';
import { RenderGrid } from './api';

const ARROW_STRIDE = 2;

export class View3d {
  private renderer: THREE.WebGLRenderer;
  private scene = new THREE.Scene();
  private camera: THREE.PerspectiveCamera;
  private zonesGroup = new THREE.Group();
  private ghostGroup = new THREE.Group();
  private raycaster = new THREE.Raycaster();
  private theta = 0.9;
  private phi = 0.9;
  private radius = 90;
  private target = new THREE.Vector3(16, 4, 16);
  onGhostClick: (p: [number, number, number]) => void = () => {};
  channel: 'both' | 'vel' | 'dye' = 'both';
  nLoc = 16;

  constructor(private el: HTMLElement) {
    this.renderer = new THREE.WebGLRenderer({ antialias: true });
    this.renderer.setPixelRatio(window.devicePixelRatio);
    el.appendChild(this.renderer.domElement);
    this.camera = new THREE.PerspectiveCamera(50, 1, 0.1, 2000);
    this.scene.background = new THREE.Color(0x0b1020);
    this.scene.add(new THREE.AmbientLight(0xffffff, 0.8));
    const d = new THREE.DirectionalLight(0xffffff, 0.8);
    d.position.set(50, 80, 30);
    this.scene.add(d, this.zonesGroup, this.ghostGroup);
    this.bindControls();
    new ResizeObserver(() => this.resize()).observe(el);
    this.resize();
    const loop = () => {
      this.updateCamera();
      this.renderer.render(this.scene, this.camera);
      requestAnimationFrame(loop);
    };
    loop();
  }

  private resize() {
    const w = this.el.clientWidth || 600;
    const h = this.el.clientHeight || 400;
    this.renderer.setSize(w, h);
    this.camera.aspect = w / h;
    this.camera.updateProjectionMatrix();
  }

  private updateCamera() {
    const t = this.target;
    this.camera.position.set(
      t.x + this.radius * Math.sin(this.phi) * Math.cos(this.theta),
      t.y + this.radius * Math.cos(this.phi),
      t.z + this.radius * Math.sin(this.phi) * Math.sin(this.theta),
    );
    this.camera.lookAt(t);
  }

  private bindControls() {
    const c = this.renderer.domElement;
    let drag: { x: number; y: number; moved: boolean } | null = null;
    c.addEventListener('pointerdown', (e) => (drag = { x: e.clientX, y: e.clientY, moved: false }));
    c.addEventListener('pointermove', (e) => {
      if (!drag) return;
      const dx = e.clientX - drag.x;
      const dy = e.clientY - drag.y;
      if (Math.abs(dx) + Math.abs(dy) > 2) drag.moved = true;
      this.theta += dx * 0.01;
      this.phi = Math.min(Math.max(this.phi - dy * 0.01, 0.15), Math.PI / 2 - 0.05);
      drag.x = e.clientX;
      drag.y = e.clientY;
    });
    c.addEventListener('pointerup', (e) => {
      if (drag && !drag.moved) this.click(e);
      drag = null;
    });
    c.addEventListener('wheel', (e) => {
      e.preventDefault();
      this.radius = Math.min(Math.max(this.radius * (1 + Math.sign(e.deltaY) * 0.1), 15), 600);
    });
  }

  private click(e: PointerEvent) {
    const r = this.renderer.domElement.getBoundingClientRect();
    const v = new THREE.Vector2(((e.clientX - r.left) / r.width) * 2 - 1, -((e.clientY - r.top) / r.height) * 2 + 1);
    this.raycaster.setFromCamera(v, this.camera);
    const hit = this.raycaster.intersectObjects(this.ghostGroup.children, false)[0];
    if (hit) this.onGhostClick(hit.object.userData.point);
  }

  /** Lattice coordinates (x, y, z with z up) to scene coordinates (y up). */
  private toScene(x: number, y: number, z: number) {
    return new THREE.Vector3(x, z, y);
  }

  setGhosts(points: [number, number, number][]) {
    this.ghostGroup.clear();
    const n = this.nLoc;
    for (const p of points) {
      const geo = new THREE.BoxGeometry(n, n, n);
      const mat = new THREE.MeshBasicMaterial({ color: 0x58a6ff, transparent: true, opacity: 0.08, depthWrite: false });
      const m = new THREE.Mesh(geo, mat);
      const c = this.toScene(p[0] * n + n / 2, p[1] * n + n / 2, p[2] * n + n / 2);
      m.position.copy(c);
      m.userData.point = p;
      const edges = new THREE.LineSegments(new THREE.EdgesGeometry(geo), new THREE.LineBasicMaterial({ color: 0x58a6ff, transparent: true, opacity: 0.35 }));
      edges.position.copy(c);
      this.ghostGroup.add(m, edges);
    }
  }

  setZones(grids: RenderGrid[]) {
    this.zonesGroup.clear();
    let maxX = 0;
    let maxY = 0;
    for (const g of grids) {
      const [nx, ny, nz] = g.dims;
      this.nLoc = nx;
      const data = g.data;
      const at = (x: number, y: number, z: number, c: number) => data[(x + nx * (y + ny * z)) * g.channels + c];
      // box outline
      const box = new THREE.LineSegments(
        new THREE.EdgesGeometry(new THREE.BoxGeometry(nx, nz, ny)),
        new THREE.LineBasicMaterial({ color: 0x334155 }),
      );
      box.position.copy(this.toScene(g.origin[0] + nx / 2, g.origin[1] + ny / 2, g.origin[2] + nz / 2));
      this.zonesGroup.add(box);
      maxX = Math.max(maxX, g.origin[0] + nx);
      maxY = Math.max(maxY, g.origin[1] + ny);
      if (this.channel !== 'dye') {
        const count = Math.ceil(nx / ARROW_STRIDE) * Math.ceil(ny / ARROW_STRIDE) * Math.ceil(nz / ARROW_STRIDE);
        const cone = new THREE.ConeGeometry(0.35, 1.6, 6).rotateX(Math.PI / 2);
        const mesh = new THREE.InstancedMesh(cone, new THREE.MeshLambertMaterial(), count);
        const m = new THREE.Matrix4();
        const q = new THREE.Quaternion();
        const fwd = new THREE.Vector3(0, 0, 1);
        const col = new THREE.Color();
        let i = 0;
        for (let z = 0; z < nz; z += ARROW_STRIDE)
          for (let y = 0; y < ny; y += ARROW_STRIDE)
            for (let x = 0; x < nx; x += ARROW_STRIDE) {
              const v = this.toScene(at(x, y, z, 0), at(x, y, z, 1), at(x, y, z, 2));
              const speed = v.length();
              q.setFromUnitVectors(fwd, speed > 1e-6 ? v.clone().normalize() : fwd);
              const s = Math.min(0.4 + speed * 2.5, 2.2);
              m.compose(this.toScene(g.origin[0] + x + 0.5, g.origin[1] + y + 0.5, g.origin[2] + z + 0.5), q, new THREE.Vector3(s, s, s));
              mesh.setMatrixAt(i, m);
              col.setHSL(0.6 - Math.min(speed * 0.8, 0.6), 0.9, 0.55);
              mesh.setColorAt(i, col);
              i++;
            }
        this.zonesGroup.add(mesh);
      }
      if (this.channel !== 'vel') {
        const pos: number[] = [];
        const cols: number[] = [];
        for (let z = 0; z < nz; z++)
          for (let y = 0; y < ny; y++)
            for (let x = 0; x < nx; x++) {
              const c = at(x, y, z, 3);
              if (c < 0.05) continue;
              const p = this.toScene(g.origin[0] + x + 0.5, g.origin[1] + y + 0.5, g.origin[2] + z + 0.5);
              pos.push(p.x, p.y, p.z);
              const k = Math.min(c / 3, 1);
              cols.push(1, 0.4 + 0.5 * (1 - k), 0.2);
            }
        const geo = new THREE.BufferGeometry();
        geo.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
        geo.setAttribute('color', new THREE.Float32BufferAttribute(cols, 3));
        this.zonesGroup.add(new THREE.Points(geo, new THREE.PointsMaterial({ size: 0.9, vertexColors: true, transparent: true, opacity: 0.7 })));
      }
    }
    if (grids.length) this.target.set(maxX / 2, 4, maxY / 2);
  }
}
