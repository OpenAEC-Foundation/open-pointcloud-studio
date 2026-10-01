/**
 * App Store — Minimal Zustand store for Open Pointcloud Studio
 *
 * Contains only UI theme state + pointcloud state.
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
// UI Theme
// ============================================================================

export type UITheme = 'dark' | 'light' | 'blue' | 'highContrast';

export const UI_THEMES: { id: UITheme; label: string }[] = [
  { id: 'dark', label: 'Dark' },
  { id: 'light', label: 'Light' },
  { id: 'blue', label: 'Blue' },
  { id: 'highContrast', label: 'High Contrast' },
];

export interface UIState {
  uiTheme: UITheme;
  rightPanelOpen: boolean;
  showBAG3DPanel: boolean;
}

export interface UIActions {
  setUITheme: (theme: UITheme) => void;
  toggleRightPanel: () => void;
  setShowBAG3DPanel: (show: boolean) => void;
}

const initialUIState: UIState = {
  uiTheme: 'dark',
  rightPanelOpen: true,
  showBAG3DPanel: false,
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
    setUITheme: (theme: UITheme) => {
      set((s) => { s.uiTheme = theme; });
    },
    toggleRightPanel: () => {
      set((s) => { s.rightPanelOpen = !s.rightPanelOpen; });
    },
    setShowBAG3DPanel: (show: boolean) => {
      set((s) => { s.showBAG3DPanel = show; });
    },

    // Pointcloud actions
    ...createPointcloudSlice(set as any, get as any),
  }))
);
