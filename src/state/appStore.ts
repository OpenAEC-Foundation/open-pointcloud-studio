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

/**
 * Theme ids match the `data-theme` values defined by the OpenAEC design
 * tokens (see styles/openaec-tokens.css). Both are dark; "light" is the
 * lighter slate surface, "openaec" the darker one.
 */
export type UITheme = 'openaec' | 'light';

export const UI_THEMES: { id: UITheme; label: string }[] = [
  { id: 'openaec', label: 'OpenAEC Dark' },
  { id: 'light', label: 'OpenAEC Slate' },
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
  uiTheme: 'openaec',
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
