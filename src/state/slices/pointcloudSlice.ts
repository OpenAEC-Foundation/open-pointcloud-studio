/**
 * Pointcloud Slice - Manages pointcloud viewer state
 *
 * Tracks loaded pointclouds, display settings (color mode, point size,
 * point budget), and Eye-Dome Lighting toggle.
 */

// ============================================================================
// Types
// ============================================================================

export interface PointcloudEntry {
  id: string;
  fileName: string;
  filePath: string;
  format: string;
  totalPoints: number;
  bounds: {
    minX: number; minY: number; minZ: number;
    maxX: number; maxY: number; maxZ: number;
  };
  hasColor: boolean;
  hasIntensity: boolean;
  hasClassification: boolean;
  visible: boolean;
  indexingProgress: number;
  indexingPhase: string;
}

export type PointcloudColorMode = 'rgb' | 'intensity' | 'elevation' | 'classification';

// ============================================================================
// State Interface
// ============================================================================

export interface PointcloudState {
  /** Loaded pointclouds */
  pointclouds: PointcloudEntry[];
  /** Currently active pointcloud ID */
  activePointcloudId: string | null;
  /** Whether the 3D pointcloud view is shown */
  showPointcloudView: boolean;
  /** Color mode for rendering */
  pointcloudColorMode: PointcloudColorMode;
  /** Point size in pixels (1-20) */
  pointcloudPointSize: number;
  /** Maximum number of points to render */
  pointBudget: number;
  /** Eye-Dome Lighting enabled */
  edlEnabled: boolean;
  /** EDL strength (0-5) */
  edlStrength: number;
  /** Visible ASPRS classification codes */
  visibleClassifications: number[];
}

// ============================================================================
// Actions Interface
// ============================================================================

export interface PointcloudActions {
  addPointcloud: (entry: PointcloudEntry) => void;
  removePointcloud: (id: string) => void;
  setActivePointcloudId: (id: string | null) => void;
  setPointcloudVisible: (id: string, visible: boolean) => void;
  updatePointcloudProgress: (id: string, progress: number, phase: string) => void;
  setShowPointcloudView: (show: boolean) => void;
  setPointcloudColorMode: (mode: PointcloudColorMode) => void;
  setPointcloudPointSize: (size: number) => void;
  setPointBudget: (budget: number) => void;
  setEdlEnabled: (enabled: boolean) => void;
  setEdlStrength: (strength: number) => void;
  setVisibleClassifications: (codes: number[]) => void;
}

export type PointcloudSlice = PointcloudState & PointcloudActions;

// ============================================================================
// Initial State
// ============================================================================

/** Default ASPRS classification codes (all visible) */
const ALL_CLASSIFICATIONS = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18];

export const initialPointcloudState: PointcloudState = {
  pointclouds: [],
  activePointcloudId: null,
  showPointcloudView: false,
  pointcloudColorMode: 'rgb',
  pointcloudPointSize: 2,
  pointBudget: 2_000_000,
  edlEnabled: true,
  edlStrength: 1.0,
  visibleClassifications: ALL_CLASSIFICATIONS,
};

// ============================================================================
// Slice Creator
// ============================================================================

interface FullStore extends PointcloudState {}

export const createPointcloudSlice = (
  set: (fn: (state: FullStore) => void) => void,
  _get: () => FullStore
): PointcloudActions => ({
  addPointcloud: (entry: PointcloudEntry) => {
    set((s) => {
      s.pointclouds.push(entry);
      if (!s.activePointcloudId) {
        s.activePointcloudId = entry.id;
      }
    });
  },

  removePointcloud: (id: string) => {
    set((s) => {
      s.pointclouds = s.pointclouds.filter((p) => p.id !== id);
      if (s.activePointcloudId === id) {
        s.activePointcloudId = s.pointclouds.length > 0 ? s.pointclouds[0].id : null;
      }
    });
  },

  setActivePointcloudId: (id: string | null) => {
    set((s) => { s.activePointcloudId = id; });
  },

  setPointcloudVisible: (id: string, visible: boolean) => {
    set((s) => {
      const pc = s.pointclouds.find((p) => p.id === id);
      if (pc) pc.visible = visible;
    });
  },

  updatePointcloudProgress: (id: string, progress: number, phase: string) => {
    set((s) => {
      const pc = s.pointclouds.find((p) => p.id === id);
      if (pc) {
        pc.indexingProgress = progress;
        pc.indexingPhase = phase;
      }
    });
  },

  setShowPointcloudView: (show: boolean) => {
    set((s) => { s.showPointcloudView = show; });
  },

  setPointcloudColorMode: (mode: PointcloudColorMode) => {
    set((s) => { s.pointcloudColorMode = mode; });
  },

  setPointcloudPointSize: (size: number) => {
    set((s) => { s.pointcloudPointSize = Math.max(0.1, Math.min(20, size)); });
  },

  setPointBudget: (budget: number) => {
    set((s) => { s.pointBudget = Math.max(100_000, Math.min(10_000_000, budget)); });
  },

  setEdlEnabled: (enabled: boolean) => {
    set((s) => { s.edlEnabled = enabled; });
  },

  setEdlStrength: (strength: number) => {
    set((s) => { s.edlStrength = Math.max(0, Math.min(5, strength)); });
  },

  setVisibleClassifications: (codes: number[]) => {
    set((s) => { s.visibleClassifications = codes; });
  },
});
