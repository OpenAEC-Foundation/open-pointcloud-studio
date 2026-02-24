/**
 * PointcloudViewer — Three.js-based 3D pointcloud viewer overlay.
 *
 * Follows BimViewer.tsx pattern: renders an independent Three.js scene
 * as an overlay on the main canvas. Uses LODController for octree-based
 * level-of-detail rendering.
 */

import { useRef, useEffect, memo } from 'react';
import * as THREE from 'three';
import { useAppStore } from '../../state/appStore';
import { LODController } from '../../engine/pointcloud/LODController';
import { getBrowserPointcloud, removeBrowserPointcloud } from '../../engine/pointcloud/BrowserPointcloudStore';
import { createPointcloudMaterial, updatePointcloudMaterial } from '../../engine/pointcloud/PointcloudMaterial';

// Dynamically import OrbitControls
let OrbitControls: any = null;

const isTauri = !!(window as any).__TAURI_INTERNALS__;

const PointcloudViewerInner = () => {
  const containerRef = useRef<HTMLDivElement>(null);
  const rendererRef = useRef<THREE.WebGLRenderer | null>(null);
  const sceneRef = useRef<THREE.Scene | null>(null);
  const cameraRef = useRef<THREE.PerspectiveCamera | null>(null);
  const controlsRef = useRef<any>(null);
  const lodControllersRef = useRef<Map<string, LODController>>(new Map());
  const browserPointsRef = useRef<Map<string, THREE.Points>>(new Map());
  const browserMaterialRef = useRef<THREE.ShaderMaterial | null>(null);
  const animFrameRef = useRef<number>(0);

  const pointclouds = useAppStore((s) => s.pointclouds);
  const colorMode = useAppStore((s) => s.pointcloudColorMode);
  const pointSize = useAppStore((s) => s.pointcloudPointSize);
  const pointBudget = useAppStore((s) => s.pointBudget);


  // Initialize Three.js scene
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    // Scene
    const scene = new THREE.Scene();
    scene.background = new THREE.Color(0x1a1a2e);
    sceneRef.current = scene;

    // Camera
    const width = container.clientWidth;
    const height = container.clientHeight;
    const camera = new THREE.PerspectiveCamera(60, width / height, 0.1, 100000);
    camera.position.set(0, 50, 100);
    camera.lookAt(0, 0, 0);
    cameraRef.current = camera;

    // Renderer
    const renderer = new THREE.WebGLRenderer({ antialias: true });
    renderer.setSize(width, height);
    renderer.setPixelRatio(window.devicePixelRatio);
    container.appendChild(renderer.domElement);
    rendererRef.current = renderer;

    // OrbitControls (from three/examples)
    import('three/examples/jsm/controls/OrbitControls.js').then((module) => {
      OrbitControls = module.OrbitControls;
      const controls = new OrbitControls(camera, renderer.domElement);
      controls.enableDamping = true;
      controls.dampingFactor = 0.1;
      controls.screenSpacePanning = true;
      controls.maxPolarAngle = Math.PI;
      controls.maxDistance = 50000;
      controls.minDistance = 0.1;
      controlsRef.current = controls;
    });

    // Ambient light for visual reference
    scene.add(new THREE.AmbientLight(0xffffff, 0.5));

    // Grid helper
    const grid = new THREE.GridHelper(100, 100, 0x444466, 0x222244);
    scene.add(grid);

    // Axes helper
    const axes = new THREE.AxesHelper(10);
    scene.add(axes);

    // Render loop
    const animate = () => {
      animFrameRef.current = requestAnimationFrame(animate);
      if (controlsRef.current) controlsRef.current.update();
      renderer.render(scene, camera);
    };
    animate();

    // Resize handler
    const onResize = () => {
      const w = container.clientWidth;
      const h = container.clientHeight;
      camera.aspect = w / h;
      camera.updateProjectionMatrix();
      renderer.setSize(w, h);
    };
    const observer = new ResizeObserver(onResize);
    observer.observe(container);

    return () => {
      cancelAnimationFrame(animFrameRef.current);
      observer.disconnect();

      // Dispose LOD controllers
      for (const ctrl of lodControllersRef.current.values()) {
        ctrl.dispose();
      }
      lodControllersRef.current.clear();

      // Dispose browser points
      for (const pts of browserPointsRef.current.values()) {
        scene.remove(pts);
        pts.geometry.dispose();
      }
      browserPointsRef.current.clear();
      if (browserMaterialRef.current) {
        browserMaterialRef.current.dispose();
        browserMaterialRef.current = null;
      }

      // Dispose Three.js resources
      if (controlsRef.current) controlsRef.current.dispose();
      renderer.dispose();
      container.removeChild(renderer.domElement);
    };
  }, []);

  // Manage pointcloud rendering when pointclouds change
  useEffect(() => {
    const scene = sceneRef.current;
    const camera = cameraRef.current;
    if (!scene) return;

    const currentIds = new Set(pointclouds.map((pc) => pc.id));

    if (isTauri) {
      // Tauri mode: use LODController
      const existingIds = new Set(lodControllersRef.current.keys());

      for (const id of existingIds) {
        if (!currentIds.has(id)) {
          const ctrl = lodControllersRef.current.get(id);
          if (ctrl) {
            ctrl.dispose();
            lodControllersRef.current.delete(id);
          }
        }
      }

      for (const pc of pointclouds) {
        if (!existingIds.has(pc.id) && pc.indexingProgress >= 1.0) {
          const ctrl = new LODController(scene, pc.id, {
            pointSize,
            colorMode,
            screenHeight: containerRef.current?.clientHeight ?? 800,
            elevationMin: pc.bounds.minZ,
            elevationMax: pc.bounds.maxZ,
          });

          const cx = (pc.bounds.minX + pc.bounds.maxX) / 2;
          const cy = (pc.bounds.minY + pc.bounds.maxY) / 2;
          const cz = (pc.bounds.minZ + pc.bounds.maxZ) / 2;
          ctrl.setWorldOffset([cx, cy, cz]);

          lodControllersRef.current.set(pc.id, ctrl);
        }
      }
    } else {
      // Browser mode: create Three.js geometry directly from parsed data
      const existingIds = new Set(browserPointsRef.current.keys());

      // Remove pointclouds that were deleted
      for (const id of existingIds) {
        if (!currentIds.has(id)) {
          const pts = browserPointsRef.current.get(id);
          if (pts) {
            scene.remove(pts);
            pts.geometry.dispose();
            browserPointsRef.current.delete(id);
            removeBrowserPointcloud(id);
          }
        }
      }

      // Add new pointclouds
      for (const pc of pointclouds) {
        if (!existingIds.has(pc.id) && pc.indexingProgress >= 1.0) {
          const parsed = getBrowserPointcloud(pc.id);
          if (!parsed) continue;

          // Create shared material if not yet created
          if (!browserMaterialRef.current) {
            browserMaterialRef.current = createPointcloudMaterial({
              pointSize,
              colorMode,
              screenHeight: containerRef.current?.clientHeight ?? 800,
              elevationMin: pc.bounds.minZ,
              elevationMax: pc.bounds.maxZ,
            });
          }

          const geometry = new THREE.BufferGeometry();
          geometry.setAttribute('position', new THREE.BufferAttribute(parsed.positions, 3));
          geometry.setAttribute('aColor', new THREE.BufferAttribute(parsed.colors, 3));
          geometry.setAttribute('aIntensity', new THREE.BufferAttribute(parsed.intensities, 1));
          geometry.setAttribute('aClassification', new THREE.BufferAttribute(parsed.classifications, 1));

          const points = new THREE.Points(geometry, browserMaterialRef.current);
          scene.add(points);
          browserPointsRef.current.set(pc.id, points);

          // Auto-fit camera to the pointcloud
          if (camera && controlsRef.current) {
            geometry.computeBoundingSphere();
            const sphere = geometry.boundingSphere;
            if (sphere) {
              const dist = sphere.radius * 2.5;
              // Update far plane based on pointcloud size
              camera.far = Math.max(camera.far, dist * 20);
              camera.updateProjectionMatrix();
              controlsRef.current.maxDistance = dist * 10;

              camera.position.set(
                sphere.center.x + dist * 0.5,
                sphere.center.y + dist * 0.7,
                sphere.center.z + dist * 0.5,
              );
              controlsRef.current.target.copy(sphere.center);
              controlsRef.current.update();
            }
          }
        }
      }
    }
  }, [pointclouds]);

  // Update visibility when toggled
  useEffect(() => {
    for (const pc of pointclouds) {
      const pts = browserPointsRef.current.get(pc.id);
      if (pts) pts.visible = pc.visible;
    }
  }, [pointclouds]);

  // Update material when settings change
  useEffect(() => {
    const opts = {
      pointSize,
      colorMode,
      screenHeight: containerRef.current?.clientHeight ?? 800,
    };

    // Tauri mode: update LOD controllers
    for (const ctrl of lodControllersRef.current.values()) {
      ctrl.updateMaterial(opts);
    }

    // Browser mode: update shared material
    if (browserMaterialRef.current) {
      updatePointcloudMaterial(browserMaterialRef.current, opts);
    }
  }, [pointSize, colorMode]);

  // LOD update loop — runs alongside render loop
  useEffect(() => {
    const camera = cameraRef.current;
    if (!camera) return;

    let running = true;
    const lodLoop = async () => {
      while (running) {
        for (const ctrl of lodControllersRef.current.values()) {
          await ctrl.update(camera, pointBudget);
        }
        // Wait ~100ms before next LOD check
        await new Promise((r) => setTimeout(r, 100));
      }
    };
    lodLoop();

    return () => { running = false; };
  }, [pointBudget]);

  return (
    <div
      ref={containerRef}
      className="absolute inset-0 z-10"
      style={{ background: '#1a1a2e' }}
    />
  );
};

export const PointcloudViewer = memo(PointcloudViewerInner);
