/**
 * PointcloudPanel — Properties panel for loaded pointclouds.
 *
 * Shows list of loaded pointclouds with visibility toggles,
 * display settings (color mode, point size, point budget),
 * classification filter, and EDL toggle.
 */

import { memo } from 'react';
import { Eye, EyeOff, Trash2 } from 'lucide-react';
import { useAppStore } from '../../state/appStore';
import type { PointcloudColorMode } from '../../state/slices/pointcloudSlice';

const ASPRS_CLASSIFICATIONS: { code: number; label: string }[] = [
  { code: 0, label: 'Never Classified' },
  { code: 1, label: 'Unassigned' },
  { code: 2, label: 'Ground' },
  { code: 3, label: 'Low Vegetation' },
  { code: 4, label: 'Medium Vegetation' },
  { code: 5, label: 'High Vegetation' },
  { code: 6, label: 'Building' },
  { code: 7, label: 'Low Point (Noise)' },
  { code: 9, label: 'Water' },
  { code: 10, label: 'Rail' },
  { code: 11, label: 'Road Surface' },
  { code: 13, label: 'Wire - Guard' },
  { code: 14, label: 'Wire - Conductor' },
  { code: 15, label: 'Transmission Tower' },
  { code: 17, label: 'Bridge Deck' },
];

const COLOR_MODES: { value: PointcloudColorMode; label: string }[] = [
  { value: 'rgb', label: 'RGB Color' },
  { value: 'intensity', label: 'Intensity' },
  { value: 'elevation', label: 'Elevation' },
  { value: 'classification', label: 'Classification' },
];

function PointcloudPanelInner() {
  const pointclouds = useAppStore((s) => s.pointclouds);
  const activePointcloudId = useAppStore((s) => s.activePointcloudId);
  const setActivePointcloudId = useAppStore((s) => s.setActivePointcloudId);
  const setPointcloudVisible = useAppStore((s) => s.setPointcloudVisible);
  const removePointcloud = useAppStore((s) => s.removePointcloud);
  const colorMode = useAppStore((s) => s.pointcloudColorMode);
  const setColorMode = useAppStore((s) => s.setPointcloudColorMode);
  const pointSize = useAppStore((s) => s.pointcloudPointSize);
  const setPointSize = useAppStore((s) => s.setPointcloudPointSize);
  const pointBudget = useAppStore((s) => s.pointBudget);
  const setPointBudget = useAppStore((s) => s.setPointBudget);
  const edlEnabled = useAppStore((s) => s.edlEnabled);
  const setEdlEnabled = useAppStore((s) => s.setEdlEnabled);
  const edlStrength = useAppStore((s) => s.edlStrength);
  const setEdlStrength = useAppStore((s) => s.setEdlStrength);
  const visibleClassifications = useAppStore((s) => s.visibleClassifications);
  const setVisibleClassifications = useAppStore((s) => s.setVisibleClassifications);

  const toggleClassification = (code: number) => {
    if (visibleClassifications.includes(code)) {
      setVisibleClassifications(visibleClassifications.filter((c) => c !== code));
    } else {
      setVisibleClassifications([...visibleClassifications, code]);
    }
  };

  return (
    <div className="flex flex-col gap-2 p-2 text-xs text-cad-text">
      {/* Loaded Pointclouds */}
      <div className="font-semibold text-cad-text-dim uppercase tracking-wide mb-1">
        Pointclouds
      </div>
      {pointclouds.length === 0 && (
        <div className="text-cad-text-muted italic">No pointclouds loaded</div>
      )}
      {pointclouds.map((pc) => (
        <div
          key={pc.id}
          className={`flex items-center gap-1 px-1 py-0.5 rounded cursor-pointer ${
            pc.id === activePointcloudId ? 'bg-cad-accent/20' : 'hover:bg-cad-hover'
          }`}
          onClick={() => setActivePointcloudId(pc.id)}
        >
          <button
            className="p-0.5 hover:bg-cad-hover rounded"
            onClick={(e) => {
              e.stopPropagation();
              setPointcloudVisible(pc.id, !pc.visible);
            }}
            title={pc.visible ? 'Hide' : 'Show'}
          >
            {pc.visible ? <Eye size={12} /> : <EyeOff size={12} />}
          </button>
          <span className="flex-1 truncate">{pc.fileName}</span>
          <span className="text-cad-text-muted">{formatPoints(pc.totalPoints)}</span>
          <button
            className="p-0.5 hover:bg-cad-hover rounded text-cad-text-muted hover:text-red-400"
            onClick={(e) => {
              e.stopPropagation();
              removePointcloud(pc.id);
            }}
            title="Remove"
          >
            <Trash2 size={12} />
          </button>
        </div>
      ))}

      {/* Display Settings */}
      <div className="border-t border-cad-border mt-2 pt-2">
        <div className="font-semibold text-cad-text-dim uppercase tracking-wide mb-1">
          Display
        </div>

        {/* Color Mode */}
        <label className="flex items-center gap-2 mb-1">
          <span className="w-16">Color:</span>
          <select
            className="flex-1 bg-cad-input text-cad-text rounded px-1 py-0.5 text-xs border border-cad-border"
            value={colorMode}
            onChange={(e) => setColorMode(e.target.value as PointcloudColorMode)}
          >
            {COLOR_MODES.map((m) => (
              <option key={m.value} value={m.value}>{m.label}</option>
            ))}
          </select>
        </label>

        {/* Point Size */}
        <label className="flex items-center gap-2 mb-1">
          <span className="w-16">Size:</span>
          <input
            type="range"
            min="1"
            max="20"
            step="1"
            value={pointSize}
            onChange={(e) => setPointSize(Number(e.target.value))}
            className="flex-1"
          />
          <span className="w-6 text-right">{pointSize}</span>
        </label>

        {/* Point Budget */}
        <label className="flex items-center gap-2 mb-1">
          <span className="w-16">Budget:</span>
          <input
            type="range"
            min="100000"
            max="10000000"
            step="100000"
            value={pointBudget}
            onChange={(e) => setPointBudget(Number(e.target.value))}
            className="flex-1"
          />
          <span className="w-10 text-right">{formatPoints(pointBudget)}</span>
        </label>

        {/* EDL */}
        <label className="flex items-center gap-2 mb-1">
          <span className="w-16">EDL:</span>
          <input
            type="checkbox"
            checked={edlEnabled}
            onChange={(e) => setEdlEnabled(e.target.checked)}
          />
          {edlEnabled && (
            <input
              type="range"
              min="0"
              max="5"
              step="0.1"
              value={edlStrength}
              onChange={(e) => setEdlStrength(Number(e.target.value))}
              className="flex-1"
            />
          )}
        </label>
      </div>

      {/* Classification Filter */}
      {colorMode === 'classification' && (
        <div className="border-t border-cad-border mt-2 pt-2">
          <div className="font-semibold text-cad-text-dim uppercase tracking-wide mb-1">
            Classification Filter
          </div>
          <div className="flex flex-col gap-0.5 max-h-48 overflow-y-auto">
            {ASPRS_CLASSIFICATIONS.map((cls) => (
              <label key={cls.code} className="flex items-center gap-1 cursor-pointer hover:bg-cad-hover px-1 rounded">
                <input
                  type="checkbox"
                  checked={visibleClassifications.includes(cls.code)}
                  onChange={() => toggleClassification(cls.code)}
                  className="w-3 h-3"
                />
                <span className="text-cad-text-muted w-4 text-right">{cls.code}</span>
                <span>{cls.label}</span>
              </label>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

function formatPoints(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(0)}K`;
  return n.toString();
}

export const PointcloudPanel = memo(PointcloudPanelInner);
