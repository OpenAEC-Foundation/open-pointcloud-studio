/**
 * Eye-Dome Lighting — screen-space shading pass for pointclouds.
 *
 * Renders the scene into an offscreen target with a depth attachment, then
 * composites it through a full-screen shader that darkens pixels whose
 * neighbours sit measurably closer to the camera. That gives points a sense
 * of relief and silhouette without any normals, which is what makes bare
 * LiDAR legible.
 *
 * The response function follows Potree: the mean positive log-depth
 * difference against a ring of neighbours, mapped through exp(-response).
 * Depth comes from a real depth attachment rather than being packed into
 * the colour target's alpha, so the pointcloud material needs no changes.
 */

import * as THREE from 'three';

/** Neighbour ring, in pixels, scaled by uRadius. */
const NEIGHBOUR_COUNT = 8;

function buildNeighbours(): THREE.Vector2[] {
  const out: THREE.Vector2[] = [];
  for (let i = 0; i < NEIGHBOUR_COUNT; i++) {
    const a = (i / NEIGHBOUR_COUNT) * Math.PI * 2;
    out.push(new THREE.Vector2(Math.cos(a), Math.sin(a)));
  }
  return out;
}

const vertexShader = /* glsl */ `
  varying vec2 vUv;

  void main() {
    vUv = uv;
    gl_Position = vec4(position.xy, 0.0, 1.0);
  }
`;

const fragmentShader = /* glsl */ `
  uniform sampler2D tColor;
  uniform sampler2D tDepth;
  uniform vec2  uResolution;
  uniform vec2  uNeighbours[${NEIGHBOUR_COUNT}];
  uniform float uRadius;
  uniform float uStrength;
  uniform float uNear;
  uniform float uFar;

  varying vec2 vUv;

  /** Perspective depth-buffer value -> positive view-space distance. */
  float viewZ(float depth) {
    float ndc = depth * 2.0 - 1.0;
    return (2.0 * uNear * uFar) / (uFar + uNear - ndc * (uFar - uNear));
  }

  void main() {
    vec4 color = texture2D(tColor, vUv);
    float centerDepth = texture2D(tDepth, vUv).x;

    // Background pixels have nothing to be shaded against.
    if (centerDepth >= 1.0) {
      gl_FragColor = color;
      return;
    }

    float logCenter = log2(viewZ(centerDepth));
    float response = 0.0;

    for (int i = 0; i < ${NEIGHBOUR_COUNT}; i++) {
      vec2 uv = clamp(vUv + uNeighbours[i] * uRadius / uResolution, vec2(0.0), vec2(1.0));
      float d = texture2D(tDepth, uv).x;
      // Treat background neighbours as infinitely far: they mark a silhouette,
      // which is exactly where EDL should darken.
      float z = d >= 1.0 ? uFar : viewZ(d);
      response += max(0.0, logCenter - log2(z));
    }

    response /= float(${NEIGHBOUR_COUNT});

    float shade = exp(-response * 300.0 * uStrength);
    gl_FragColor = vec4(color.rgb * shade, color.a);
  }
`;

export interface EDLOptions {
  /** 0 disables shading entirely; the UI exposes 0–5. */
  strength?: number;
  /** Neighbour sampling radius in pixels. */
  radius?: number;
}

export class EDLPass {
  private renderer: THREE.WebGLRenderer;
  private target: THREE.WebGLRenderTarget;
  private material: THREE.ShaderMaterial;
  private quad: THREE.Mesh;
  private quadScene: THREE.Scene;
  private quadCamera: THREE.OrthographicCamera;
  private disposed = false;

  constructor(renderer: THREE.WebGLRenderer) {
    this.renderer = renderer;

    const size = renderer.getDrawingBufferSize(new THREE.Vector2());
    const width = Math.max(1, size.x);
    const height = Math.max(1, size.y);

    const depthTexture = new THREE.DepthTexture(width, height);
    depthTexture.format = THREE.DepthFormat;
    depthTexture.type = THREE.UnsignedIntType;
    depthTexture.minFilter = THREE.NearestFilter;
    depthTexture.magFilter = THREE.NearestFilter;

    this.target = new THREE.WebGLRenderTarget(width, height, {
      minFilter: THREE.NearestFilter,
      magFilter: THREE.NearestFilter,
      depthBuffer: true,
      depthTexture,
    });

    this.material = new THREE.ShaderMaterial({
      vertexShader,
      fragmentShader,
      uniforms: {
        tColor: { value: this.target.texture },
        tDepth: { value: this.target.depthTexture },
        uResolution: { value: new THREE.Vector2(width, height) },
        uNeighbours: { value: buildNeighbours() },
        uRadius: { value: 1.4 },
        uStrength: { value: 1.0 },
        uNear: { value: 0.1 },
        uFar: { value: 1000 },
      },
      depthTest: false,
      depthWrite: false,
    });

    // A single full-screen triangle; the vertex shader ignores the matrices.
    this.quad = new THREE.Mesh(new THREE.PlaneGeometry(2, 2), this.material);
    this.quad.frustumCulled = false;
    this.quadScene = new THREE.Scene();
    this.quadScene.add(this.quad);
    this.quadCamera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0, 1);
  }

  /** Match the offscreen target to the renderer's current drawing buffer. */
  setSize(width: number, height: number): void {
    if (this.disposed) return;
    const w = Math.max(1, Math.floor(width));
    const h = Math.max(1, Math.floor(height));
    if (this.target.width === w && this.target.height === h) return;
    this.target.setSize(w, h);
    this.material.uniforms.uResolution.value.set(w, h);
  }

  render(
    scene: THREE.Scene,
    camera: THREE.PerspectiveCamera,
    options: EDLOptions = {},
  ): void {
    if (this.disposed) return;

    const size = this.renderer.getDrawingBufferSize(new THREE.Vector2());
    this.setSize(size.x, size.y);

    const uniforms = this.material.uniforms;
    uniforms.uStrength.value = options.strength ?? 1.0;
    uniforms.uRadius.value = options.radius ?? 1.4;
    uniforms.uNear.value = camera.near;
    uniforms.uFar.value = camera.far;

    const previousTarget = this.renderer.getRenderTarget();
    this.renderer.setRenderTarget(this.target);
    this.renderer.clear();
    this.renderer.render(scene, camera);
    this.renderer.setRenderTarget(previousTarget);
    this.renderer.render(this.quadScene, this.quadCamera);
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.target.depthTexture?.dispose();
    this.target.dispose();
    this.quad.geometry.dispose();
    this.material.dispose();
  }
}
