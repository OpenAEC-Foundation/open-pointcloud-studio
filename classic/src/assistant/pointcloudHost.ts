/**
 * The app behind the assistant tools (siteAppTools.ts): the Zustand store, the parsed clouds and the live viewer.
 */
import { useAppStore } from '../state/appStore';
import { getBrowserPointcloud } from '../engine/pointcloud/BrowserPointcloudStore';
import { getViewer } from '../components/canvas/PointcloudViewer';
import type { PointcloudHost, Vec3 } from './siteAppTools';

export function createPointcloudHost(): PointcloudHost {
  const store = () => useAppStore.getState();
  return {
    state: store,
    data: id => getBrowserPointcloud(id),
    setColorMode: mode => store().setPointcloudColorMode(mode),
    setPointSize: size => store().setPointcloudPointSize(size),
    setVisible: (id, visible) => store().setPointcloudVisible(id, visible),
    setActive: id => store().setActivePointcloudId(id),
    setClipBox: box => store().setClipBox(box),
    camera() {
      const viewer = getViewer();
      if (!viewer) return null;
      const target = viewer.controls?.target ?? { x: 0, y: 0, z: 0 };
      return { position: viewer.camera.position.toArray() as Vec3, target: [target.x, target.y, target.z], fov: viewer.camera.fov };
    },
    setCamera(position, target) {
      const viewer = getViewer();
      if (!viewer) return false;
      const { camera, controls } = viewer;
      const distance = Math.hypot(position[0] - target[0], position[1] - target[1], position[2] - target[2]);
      camera.near = Math.max(0.01, distance * 0.001);
      camera.far = Math.max(camera.far, distance * 20);
      camera.updateProjectionMatrix();
      camera.position.set(...position);
      if (controls) {
        controls.maxDistance = Math.max(controls.maxDistance, distance * 10);
        controls.target.set(...target);
        controls.update();
      } else camera.lookAt(...target);
      return true;
    },
    capture(max, type) {
      const viewer = getViewer();
      if (!viewer) return null;
      const source = viewer.renderer.domElement;
      // Without preserveDrawingBuffer the canvas is only readable right after a render.
      viewer.renderer.render(viewer.scene, viewer.camera);
      const scale = Math.min(1, max / Math.max(source.width, source.height, 1));
      const canvas = document.createElement('canvas');
      canvas.width = Math.max(1, Math.round(source.width * scale)); canvas.height = Math.max(1, Math.round(source.height * scale));
      const ctx = canvas.getContext('2d');
      if (!ctx) return null;
      ctx.drawImage(source, 0, 0, canvas.width, canvas.height);
      return canvas.toDataURL(type, 0.85);
    },
  };
}
