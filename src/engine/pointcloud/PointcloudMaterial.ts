/**
 * Custom ShaderMaterial for pointcloud rendering.
 *
 * Supports multiple color modes (RGB, intensity, elevation, classification),
 * distance-based point size attenuation, and Eye-Dome Lighting preparation.
 */

import * as THREE from 'three';
import type { PointcloudColorMode } from '../../state/slices/pointcloudSlice';

const vertexShader = /* glsl */ `
  uniform float uPointSize;
  uniform float uScreenHeight;
  uniform float uBaseSpacing;   // global avg point spacing: sqrt(footprintArea / totalPoints)
  uniform int uColorMode; // 0=rgb, 1=intensity, 2=elevation, 3=classification
  uniform float uElevationMin;
  uniform float uElevationMax;

  attribute vec3 aColor;        // RGB (0-255 normalized to 0-1)
  attribute float aIntensity;   // 0-65535 normalized to 0-1
  attribute float aClassification;
  attribute float aSelected;    // 1.0 if selected, 0.0 otherwise

  varying vec3 vColor;

  // Elevation color ramp: blue -> cyan -> green -> yellow -> red
  vec3 elevationColor(float t) {
    t = clamp(t, 0.0, 1.0);
    if (t < 0.25) {
      return mix(vec3(0.0, 0.0, 1.0), vec3(0.0, 1.0, 1.0), t / 0.25);
    } else if (t < 0.5) {
      return mix(vec3(0.0, 1.0, 1.0), vec3(0.0, 1.0, 0.0), (t - 0.25) / 0.25);
    } else if (t < 0.75) {
      return mix(vec3(0.0, 1.0, 0.0), vec3(1.0, 1.0, 0.0), (t - 0.5) / 0.25);
    } else {
      return mix(vec3(1.0, 1.0, 0.0), vec3(1.0, 0.0, 0.0), (t - 0.75) / 0.25);
    }
  }

  // ASPRS classification colors
  vec3 classificationColor(float cls) {
    int c = int(cls);
    if (c == 2) return vec3(0.65, 0.49, 0.24); // Ground - brown
    if (c == 3) return vec3(0.0, 0.8, 0.0);     // Low Vegetation - green
    if (c == 4) return vec3(0.0, 0.6, 0.0);     // Medium Vegetation - darker green
    if (c == 5) return vec3(0.0, 0.4, 0.0);     // High Vegetation - dark green
    if (c == 6) return vec3(1.0, 0.0, 0.0);     // Building - red
    if (c == 7) return vec3(1.0, 0.5, 0.0);     // Low Point (noise) - orange
    if (c == 9) return vec3(0.0, 0.0, 1.0);     // Water - blue
    if (c == 17) return vec3(0.5, 0.5, 0.5);    // Bridge Deck - gray
    return vec3(0.8, 0.8, 0.8);                  // Default - light gray
  }

  void main() {
    vec4 mvPosition = modelViewMatrix * vec4(position, 1.0);

    // World-space point size from global dataset spacing
    // uBaseSpacing = sqrt(footprintArea / totalPoints), the avg distance between points
    float worldSize = uBaseSpacing * uPointSize;

    // Project to screen pixels: projectionMatrix[1][1] = 1/tan(fov/2)
    gl_PointSize = worldSize * (uScreenHeight / 2.0) * projectionMatrix[1][1] / (-mvPosition.z);
    gl_PointSize = clamp(gl_PointSize, 1.0, 32.0);

    gl_Position = projectionMatrix * mvPosition;

    // Select color based on mode
    if (uColorMode == 0) {
      vColor = aColor;
    } else if (uColorMode == 1) {
      vColor = vec3(aIntensity);
    } else if (uColorMode == 2) {
      float range = uElevationMax - uElevationMin;
      float t = range > 0.0 ? (position.y - uElevationMin) / range : 0.5;
      // Note: position.y here is relative to chunk center, actual elevation
      // is calculated with modelMatrix translation
      vec4 worldPos = modelMatrix * vec4(position, 1.0);
      t = range > 0.0 ? (worldPos.y - uElevationMin) / range : 0.5;
      vColor = elevationColor(t);
    } else if (uColorMode == 3) {
      vColor = classificationColor(aClassification);
    } else {
      vColor = aColor;
    }

    // Highlight selected points in orange
    if (aSelected > 0.5) {
      vColor = mix(vColor, vec3(1.0, 0.7, 0.0), 0.6);
    }
  }
`;

const fragmentShader = /* glsl */ `
  varying vec3 vColor;

  void main() {
    // Round point shape
    vec2 coord = gl_PointCoord - vec2(0.5);
    if (dot(coord, coord) > 0.25) discard;

    gl_FragColor = vec4(vColor, 1.0);
  }
`;

export interface PointcloudMaterialOptions {
  pointSize?: number;
  colorMode?: PointcloudColorMode;
  elevationMin?: number;
  elevationMax?: number;
  screenHeight?: number;
  baseSpacing?: number;
}

const COLOR_MODE_MAP: Record<PointcloudColorMode, number> = {
  rgb: 0,
  intensity: 1,
  elevation: 2,
  classification: 3,
};

export function createPointcloudMaterial(options: PointcloudMaterialOptions = {}): THREE.ShaderMaterial {
  return new THREE.ShaderMaterial({
    vertexShader,
    fragmentShader,
    uniforms: {
      uPointSize: { value: options.pointSize ?? 1.0 },
      uScreenHeight: { value: options.screenHeight ?? 800 },
      uBaseSpacing: { value: options.baseSpacing ?? 0.1 },
      uColorMode: { value: COLOR_MODE_MAP[options.colorMode ?? 'rgb'] },
      uElevationMin: { value: options.elevationMin ?? 0 },
      uElevationMax: { value: options.elevationMax ?? 100 },
    },
    transparent: false,
    depthTest: true,
    depthWrite: true,
  });
}

export function updatePointcloudMaterial(
  material: THREE.ShaderMaterial,
  options: Partial<PointcloudMaterialOptions>
): void {
  if (options.pointSize !== undefined) material.uniforms.uPointSize.value = options.pointSize;
  if (options.screenHeight !== undefined) material.uniforms.uScreenHeight.value = options.screenHeight;
  if (options.baseSpacing !== undefined) material.uniforms.uBaseSpacing.value = options.baseSpacing;
  if (options.colorMode !== undefined) material.uniforms.uColorMode.value = COLOR_MODE_MAP[options.colorMode];
  if (options.elevationMin !== undefined) material.uniforms.uElevationMin.value = options.elevationMin;
  if (options.elevationMax !== undefined) material.uniforms.uElevationMax.value = options.elevationMax;
}
