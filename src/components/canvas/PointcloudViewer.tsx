/**
 * PointcloudViewer — Three.js-based 3D pointcloud viewer overlay.
 *
 * Follows BimViewer.tsx pattern: renders an independent Three.js scene
 * as an overlay on the main canvas. Uses LODController for octree-based
 * level-of-detail rendering.
 *
 * Supports edit mode with box selection and point deletion.
 */

import { useRef, useEffect, useState, useCallback, memo } from 'react';
import * as THREE from 'three';
import { useAppStore } from '../../state/appStore';
import { LODController } from '../../engine/pointcloud/LODController';
import { getBrowserPointcloud, removeBrowserPointcloud } from '../../engine/pointcloud/BrowserPointcloudStore';
import { createPointcloudMaterial, updatePointcloudMaterial } from '../../engine/pointcloud/PointcloudMaterial';

// Dynamically import OrbitControls
let OrbitControls: any = null;

// Global scene reference for BAG3D mesh injection
let _viewerScene: THREE.Scene | null = null;
let _viewerCamera: THREE.PerspectiveCamera | null = null;
let _viewerControls: any = null;
let _meshMaterialRef: { current: THREE.MeshStandardMaterial | null } = { current: null };
let _bag3dMeshes: THREE.Mesh[] = [];

export function addBAG3DMeshToScene(geometry: THREE.BufferGeometry): void {
  if (!_viewerScene) return;

  if (!_meshMaterialRef.current) {
    _meshMaterialRef.current = new THREE.MeshStandardMaterial({
      vertexColors: true,
      side: THREE.DoubleSide,
      metalness: 0.1,
      roughness: 0.8,
    });
  }

  const mesh = new THREE.Mesh(geometry, _meshMaterialRef.current);
  _viewerScene.add(mesh);
  _bag3dMeshes.push(mesh);

  // Auto-fit camera to the new mesh
  geometry.computeBoundingSphere();
  const sphere = geometry.boundingSphere;
  if (sphere && _viewerCamera && _viewerControls) {
    const dist = sphere.radius * 2.5;
    _viewerCamera.far = Math.max(_viewerCamera.far, dist * 20);
    _viewerCamera.updateProjectionMatrix();
    _viewerControls.maxDistance = Math.max(_viewerControls.maxDistance, dist * 10);
    _viewerCamera.position.set(
      sphere.center.x + dist * 0.5,
      sphere.center.y + dist * 0.7,
      sphere.center.z + dist * 0.5,
    );
    _viewerControls.target.copy(sphere.center);
    _viewerControls.update();
  }
}

const isTauri = !!(window as any).__TAURI_INTERNALS__;

interface BoxSelectState {
  active: boolean;
  startX: number;
  startY: number;
  currentX: number;
  currentY: number;
}

const PointcloudViewerInner = () => {
  const containerRef = useRef<HTMLDivElement>(null);
  const rendererRef = useRef<THREE.WebGLRenderer | null>(null);
  const sceneRef = useRef<THREE.Scene | null>(null);
  const cameraRef = useRef<THREE.PerspectiveCamera | null>(null);
  const controlsRef = useRef<any>(null);
  const lodControllersRef = useRef<Map<string, LODController>>(new Map());
  const browserPointsRef = useRef<Map<string, THREE.Points>>(new Map());
  const browserMeshesRef = useRef<Map<string, THREE.Mesh>>(new Map());
  const browserMaterialRef = useRef<THREE.ShaderMaterial | null>(null);
  const browserMeshMaterialRef = useRef<THREE.MeshStandardMaterial | null>(null);
  const transformVersionsRef = useRef<Map<string, number>>(new Map());
  const animFrameRef = useRef<number>(0);

  const pointclouds = useAppStore((s) => s.pointclouds);
  const colorMode = useAppStore((s) => s.pointcloudColorMode);
  const pointSize = useAppStore((s) => s.pointcloudPointSize);
  const pointBudget = useAppStore((s) => s.pointBudget);
  const editMode = useAppStore((s) => s.editMode);
  const selectedPointIndices = useAppStore((s) => s.selectedPointIndices);

  const [boxSelect, setBoxSelect] = useState<BoxSelectState>({
    active: false, startX: 0, startY: 0, currentX: 0, currentY: 0,
  });

  // Enable/disable OrbitControls based on editMode
  useEffect(() => {
    if (controlsRef.current) {
      controlsRef.current.enabled = !editMode;
    }
  }, [editMode]);

  // Update aSelected attribute when selection changes
  useEffect(() => {
    for (const [pcId, pts] of browserPointsRef.current.entries()) {
      const geo = pts.geometry;
      const posAttr = geo.getAttribute('position');
      if (!posAttr) continue;
      const numPoints = posAttr.count;

      let selAttr = geo.getAttribute('aSelected') as THREE.BufferAttribute | null;
      if (!selAttr) {
        selAttr = new THREE.BufferAttribute(new Float32Array(numPoints), 1);
        geo.setAttribute('aSelected', selAttr);
      }

      const selected = selectedPointIndices[pcId];
      const arr = selAttr.array as Float32Array;
      arr.fill(0);
      if (selected) {
        for (const idx of selected) {
          if (idx < numPoints) arr[idx] = 1.0;
        }
      }
      selAttr.needsUpdate = true;
    }
  }, [selectedPointIndices]);

  // Box selection handlers
  const handlePointerDown = useCallback((e: React.PointerEvent) => {
    if (!editMode) return;
    if (e.button !== 0) return; // left click only
    const rect = containerRef.current!.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;
    setBoxSelect({ active: true, startX: x, startY: y, currentX: x, currentY: y });
  }, [editMode]);

  const handlePointerMove = useCallback((e: React.PointerEvent) => {
    if (!boxSelect.active) return;
    const rect = containerRef.current!.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;
    setBoxSelect((prev) => ({ ...prev, currentX: x, currentY: y }));
  }, [boxSelect.active]);

  const handlePointerUp = useCallback(() => {
    if (!boxSelect.active) return;
    const camera = cameraRef.current;
    const container = containerRef.current;
    if (!camera || !container) {
      setBoxSelect((prev) => ({ ...prev, active: false }));
      return;
    }

    const w = container.clientWidth;
    const h = container.clientHeight;

    // Compute NDC box from pixel coordinates
    const x1 = Math.min(boxSelect.startX, boxSelect.currentX);
    const x2 = Math.max(boxSelect.startX, boxSelect.currentX);
    const y1 = Math.min(boxSelect.startY, boxSelect.currentY);
    const y2 = Math.max(boxSelect.startY, boxSelect.currentY);

    // Ignore tiny drags (< 4px)
    if (x2 - x1 < 4 && y2 - y1 < 4) {
      setBoxSelect((prev) => ({ ...prev, active: false }));
      return;
    }

    // Convert to NDC (-1 to 1)
    const ndcLeft = (x1 / w) * 2 - 1;
    const ndcRight = (x2 / w) * 2 - 1;
    const ndcTop = -(y1 / h) * 2 + 1;
    const ndcBottom = -(y2 / h) * 2 + 1;

    // Build view-projection matrix
    const vpMatrix = new THREE.Matrix4();
    vpMatrix.multiplyMatrices(camera.projectionMatrix, camera.matrixWorldInverse);

    const projected = new THREE.Vector4();

    // Select points for each pointcloud
    for (const [pcId, pts] of browserPointsRef.current.entries()) {
      const posAttr = pts.geometry.getAttribute('position');
      if (!posAttr) continue;
      const positions = posAttr.array as Float32Array;
      const numPoints = posAttr.count;
      const selected: number[] = [];

      // Combine model matrix with VP
      const mvpMatrix = new THREE.Matrix4();
      mvpMatrix.multiplyMatrices(vpMatrix, pts.matrixWorld);

      for (let i = 0; i < numPoints; i++) {
        const px = positions[i * 3];
        const py = positions[i * 3 + 1];
        const pz = positions[i * 3 + 2];

        projected.set(px, py, pz, 1.0);
        projected.applyMatrix4(mvpMatrix);

        // Perspective divide
        if (projected.w <= 0) continue;
        const ndcX = projected.x / projected.w;
        const ndcY = projected.y / projected.w;

        if (ndcX >= ndcLeft && ndcX <= ndcRight && ndcY >= ndcBottom && ndcY <= ndcTop) {
          selected.push(i);
        }
      }

      if (selected.length > 0) {
        useAppStore.getState().setSelectedPoints(pcId, selected);
      }
    }

    setBoxSelect((prev) => ({ ...prev, active: false }));
  }, [boxSelect]);

  // Delete selected points
  const deleteSelectedPoints = useCallback(() => {
    const indices = useAppStore.getState().selectedPointIndices;
    const scene = sceneRef.current;
    if (!scene) return;

    for (const [pcId, selectedIdx] of Object.entries(indices)) {
      if (!selectedIdx || selectedIdx.length === 0) continue;
      const pts = browserPointsRef.current.get(pcId);
      if (!pts) continue;

      const geo = pts.geometry;
      const posAttr = geo.getAttribute('position');
      if (!posAttr) continue;

      const oldPositions = posAttr.array as Float32Array;
      const numPoints = posAttr.count;

      // Build a set of indices to remove
      const removeSet = new Set(selectedIdx);
      const keepCount = numPoints - removeSet.size;
      if (keepCount <= 0) {
        // Remove entire pointcloud
        scene.remove(pts);
        geo.dispose();
        browserPointsRef.current.delete(pcId);
        removeBrowserPointcloud(pcId);
        useAppStore.getState().removePointcloud(pcId);
        continue;
      }

      // Build new arrays
      const newPositions = new Float32Array(keepCount * 3);
      const oldColors = (geo.getAttribute('aColor')?.array as Float32Array) || null;
      const newColors = oldColors ? new Float32Array(keepCount * 3) : null;
      const oldIntensities = (geo.getAttribute('aIntensity')?.array as Float32Array) || null;
      const newIntensities = oldIntensities ? new Float32Array(keepCount) : null;
      const oldClassifications = (geo.getAttribute('aClassification')?.array as Float32Array) || null;
      const newClassifications = oldClassifications ? new Float32Array(keepCount) : null;

      let writeIdx = 0;
      for (let i = 0; i < numPoints; i++) {
        if (removeSet.has(i)) continue;
        newPositions[writeIdx * 3] = oldPositions[i * 3];
        newPositions[writeIdx * 3 + 1] = oldPositions[i * 3 + 1];
        newPositions[writeIdx * 3 + 2] = oldPositions[i * 3 + 2];
        if (newColors && oldColors) {
          newColors[writeIdx * 3] = oldColors[i * 3];
          newColors[writeIdx * 3 + 1] = oldColors[i * 3 + 1];
          newColors[writeIdx * 3 + 2] = oldColors[i * 3 + 2];
        }
        if (newIntensities && oldIntensities) {
          newIntensities[writeIdx] = oldIntensities[i];
        }
        if (newClassifications && oldClassifications) {
          newClassifications[writeIdx] = oldClassifications[i];
        }
        writeIdx++;
      }

      // Replace geometry attributes
      geo.setAttribute('position', new THREE.BufferAttribute(newPositions, 3));
      if (newColors) geo.setAttribute('aColor', new THREE.BufferAttribute(newColors, 3));
      if (newIntensities) geo.setAttribute('aIntensity', new THREE.BufferAttribute(newIntensities, 1));
      if (newClassifications) geo.setAttribute('aClassification', new THREE.BufferAttribute(newClassifications, 1));

      // Reset aSelected
      geo.setAttribute('aSelected', new THREE.BufferAttribute(new Float32Array(keepCount), 1));

      geo.computeBoundingSphere();

      // Update point count in store
      useAppStore.setState((s) => {
        const pc = s.pointclouds.find((p) => p.id === pcId);
        if (pc) pc.totalPoints = keepCount;
      });
    }

    useAppStore.getState().clearSelection();
  }, []);

  // Keyboard handlers
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && editMode) {
        useAppStore.getState().setEditMode(false);
      }
      if (e.key === 'Delete' && editMode) {
        deleteSelectedPoints();
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [editMode, deleteSelectedPoints]);

  // Initialize Three.js scene
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    // Scene
    const scene = new THREE.Scene();
    scene.background = new THREE.Color(0x1a1a2e);
    sceneRef.current = scene;
    _viewerScene = scene;

    // Camera
    const width = container.clientWidth;
    const height = container.clientHeight;
    const camera = new THREE.PerspectiveCamera(60, width / height, 0.1, 100000);
    camera.position.set(0, 50, 100);
    camera.lookAt(0, 0, 0);
    cameraRef.current = camera;
    _viewerCamera = camera;

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
      _viewerControls = controls;

      // Apply current editMode state
      controls.enabled = !useAppStore.getState().editMode;
    });

    // Lighting for mesh rendering and visual reference
    scene.add(new THREE.AmbientLight(0xffffff, 0.6));
    const dirLight = new THREE.DirectionalLight(0xffffff, 0.8);
    dirLight.position.set(1, 2, 1).normalize();
    scene.add(dirLight);
    const dirLight2 = new THREE.DirectionalLight(0xffffff, 0.3);
    dirLight2.position.set(-1, -0.5, -1).normalize();
    scene.add(dirLight2);

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
      // Dispose browser meshes
      for (const mesh of browserMeshesRef.current.values()) {
        scene.remove(mesh);
        mesh.geometry.dispose();
      }
      browserMeshesRef.current.clear();
      if (browserMaterialRef.current) {
        browserMaterialRef.current.dispose();
        browserMaterialRef.current = null;
      }
      if (browserMeshMaterialRef.current) {
        browserMeshMaterialRef.current.dispose();
        browserMeshMaterialRef.current = null;
      }
      // Dispose BAG3D meshes
      for (const mesh of _bag3dMeshes) {
        scene.remove(mesh);
        mesh.geometry.dispose();
      }
      _bag3dMeshes = [];
      _viewerScene = null;
      _viewerCamera = null;
      _viewerControls = null;

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
      const existingPointIds = new Set(browserPointsRef.current.keys());
      const existingMeshIds = new Set(browserMeshesRef.current.keys());

      // Remove pointclouds/meshes that were deleted
      for (const id of existingPointIds) {
        if (!currentIds.has(id)) {
          const pts = browserPointsRef.current.get(id);
          if (pts) {
            scene.remove(pts);
            pts.geometry.dispose();
            browserPointsRef.current.delete(id);
          }
          removeBrowserPointcloud(id);
        }
      }
      for (const id of existingMeshIds) {
        if (!currentIds.has(id)) {
          const mesh = browserMeshesRef.current.get(id);
          if (mesh) {
            scene.remove(mesh);
            mesh.geometry.dispose();
            browserMeshesRef.current.delete(id);
          }
          removeBrowserPointcloud(id);
        }
      }

      // Add new pointclouds / meshes
      for (const pc of pointclouds) {
        const alreadyExists = existingPointIds.has(pc.id) || existingMeshIds.has(pc.id);
        if (!alreadyExists && pc.indexingProgress >= 1.0) {
          const parsed = getBrowserPointcloud(pc.id);
          if (!parsed) continue;

          const hasMesh = parsed.indices && parsed.indices.length > 0;
          let fitGeometry: THREE.BufferGeometry | null = null;

          if (hasMesh) {
            // Render as mesh with vertex colors
            const geometry = new THREE.BufferGeometry();
            geometry.setAttribute('position', new THREE.BufferAttribute(parsed.positions, 3));
            geometry.setAttribute('color', new THREE.BufferAttribute(parsed.colors, 3));
            geometry.setIndex(new THREE.BufferAttribute(parsed.indices!, 1));
            geometry.computeVertexNormals();

            if (!browserMeshMaterialRef.current) {
              browserMeshMaterialRef.current = new THREE.MeshStandardMaterial({
                vertexColors: true,
                side: THREE.DoubleSide,
                metalness: 0.1,
                roughness: 0.8,
              });
            }

            const mesh = new THREE.Mesh(geometry, browserMeshMaterialRef.current);
            scene.add(mesh);
            browserMeshesRef.current.set(pc.id, mesh);
            fitGeometry = geometry;
          } else {
            // Render as point cloud
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

            const numPoints = parsed.positions.length / 3;
            geometry.setAttribute('aSelected', new THREE.BufferAttribute(new Float32Array(numPoints), 1));

            const points = new THREE.Points(geometry, browserMaterialRef.current);
            scene.add(points);
            browserPointsRef.current.set(pc.id, points);
            fitGeometry = geometry;
          }

          // Auto-fit camera
          if (camera && controlsRef.current && fitGeometry) {
            fitGeometry.computeBoundingSphere();
            const sphere = fitGeometry.boundingSphere;
            if (sphere) {
              const dist = sphere.radius * 2.5;
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

  // Rebuild geometry when transformVersion changes (translate/scale/thin/reconstruct)
  useEffect(() => {
    const scene = sceneRef.current;
    if (!scene) return;

    for (const pc of pointclouds) {
      const prevVersion = transformVersionsRef.current.get(pc.id) || 0;
      const currentVersion = pc.transformVersion || 0;

      if (currentVersion <= prevVersion) continue;
      transformVersionsRef.current.set(pc.id, currentVersion);

      const parsed = getBrowserPointcloud(pc.id);
      if (!parsed) continue;

      const hasMesh = parsed.indices && parsed.indices.length > 0;

      // Remove old point cloud or mesh for this ID
      const oldPts = browserPointsRef.current.get(pc.id);
      if (oldPts) {
        scene.remove(oldPts);
        oldPts.geometry.dispose();
        browserPointsRef.current.delete(pc.id);
      }
      const oldMesh = browserMeshesRef.current.get(pc.id);
      if (oldMesh) {
        scene.remove(oldMesh);
        oldMesh.geometry.dispose();
        browserMeshesRef.current.delete(pc.id);
      }

      if (hasMesh) {
        // Rebuild as mesh
        const geometry = new THREE.BufferGeometry();
        geometry.setAttribute('position', new THREE.BufferAttribute(parsed.positions, 3));
        geometry.setAttribute('color', new THREE.BufferAttribute(parsed.colors, 3));
        geometry.setIndex(new THREE.BufferAttribute(parsed.indices!, 1));
        geometry.computeVertexNormals();

        if (!browserMeshMaterialRef.current) {
          browserMeshMaterialRef.current = new THREE.MeshStandardMaterial({
            vertexColors: true,
            side: THREE.DoubleSide,
            metalness: 0.1,
            roughness: 0.8,
          });
        }

        const mesh = new THREE.Mesh(geometry, browserMeshMaterialRef.current);
        scene.add(mesh);
        browserMeshesRef.current.set(pc.id, mesh);
      } else {
        // Rebuild as point cloud
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
        const numPoints = parsed.positions.length / 3;
        geometry.setAttribute('aSelected', new THREE.BufferAttribute(new Float32Array(numPoints), 1));

        const points = new THREE.Points(geometry, browserMaterialRef.current);
        scene.add(points);
        browserPointsRef.current.set(pc.id, points);
      }
    }
  }, [pointclouds]);

  // Update visibility when toggled
  useEffect(() => {
    for (const pc of pointclouds) {
      const pts = browserPointsRef.current.get(pc.id);
      if (pts) pts.visible = pc.visible;
      const mesh = browserMeshesRef.current.get(pc.id);
      if (mesh) mesh.visible = pc.visible;
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

  // Compute box overlay rect
  const boxRect = boxSelect.active ? {
    left: Math.min(boxSelect.startX, boxSelect.currentX),
    top: Math.min(boxSelect.startY, boxSelect.currentY),
    width: Math.abs(boxSelect.currentX - boxSelect.startX),
    height: Math.abs(boxSelect.currentY - boxSelect.startY),
  } : null;

  return (
    <div
      ref={containerRef}
      className="absolute inset-0 z-10"
      style={{
        background: '#1a1a2e',
        cursor: editMode ? 'crosshair' : 'default',
      }}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
    >
      {/* Box selection overlay */}
      {boxRect && boxRect.width > 2 && boxRect.height > 2 && (
        <div
          style={{
            position: 'absolute',
            left: boxRect.left,
            top: boxRect.top,
            width: boxRect.width,
            height: boxRect.height,
            border: '2px dashed rgba(255, 180, 0, 0.8)',
            backgroundColor: 'rgba(255, 180, 0, 0.1)',
            pointerEvents: 'none',
            zIndex: 20,
          }}
        />
      )}
    </div>
  );
};

export const PointcloudViewer = memo(PointcloudViewerInner);
