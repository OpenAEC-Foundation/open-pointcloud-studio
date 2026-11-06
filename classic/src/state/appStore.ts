/**
 * App Store — Minimal Zustand store for Open Pointcloud Studio
 *
 * Contains only UI state + pointcloud state.
 */

import { create } from 'zustand';
import { immer } from 'zustand/middleware/immer';

import {
  type PointcloudState,
  type PointcloudActions,
  initialPointcloudState,
  createPointcloudSlice,
} from './slices';

// ============================================================================
// UI State
// ============================================================================

export interface UIState {
  rightPanelOpen: boolean;
  showBAG3DPanel: boolean;
  appMenuOpen: boolean;
}

export interface UIActions {
  toggleRightPanel: () => void;
  setShowBAG3DPanel: (show: boolean) => void;
  setAppMenuOpen: (open: boolean) => void;
}

const initialUIState: UIState = {
  rightPanelOpen: true,
  showBAG3DPanel: false,
  appMenuOpen: false,
};

// ============================================================================
// Combined State
// ============================================================================

export type AppState = UIState & UIActions & PointcloudState & PointcloudActions;

export const useAppStore = create<AppState>()(
  immer((set, get) => ({
    ...initialUIState,
    ...initialPointcloudState,

    // UI actions
    toggleRightPanel: () => {
      set((s) => { s.rightPanelOpen = !s.rightPanelOpen; });
    },
    setShowBAG3DPanel: (show: boolean) => {
      set((s) => { s.showBAG3DPanel = show; });
    },
    setAppMenuOpen: (open: boolean) => {
      set((s) => { s.appMenuOpen = open; });
    },

    // Pointcloud actions
    ...createPointcloudSlice(set as any, get as any),
  }))
);
