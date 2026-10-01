import { memo } from 'react';
import { useAppStore } from '../../../state/appStore';
import { formatPoints } from '../../../utils/format';

export const StatusBar = memo(function StatusBar() {
  const pointclouds = useAppStore((s) => s.pointclouds);
  const pointBudget = useAppStore((s) => s.pointBudget);
  const editMode = useAppStore((s) => s.editMode);
  const selectedPointIndices = useAppStore((s) => s.selectedPointIndices);

  const totalPoints = pointclouds.reduce((sum, pc) => sum + pc.totalPoints, 0);
  const totalSelected = Object.values(selectedPointIndices).reduce((sum, arr) => sum + arr.length, 0);

  return (
    <div className="h-6 bg-cad-surface border-t border-cad-border flex items-center px-3 text-xs text-cad-text-dim gap-6">
      {/* Edit mode indicator */}
      {editMode && (
        <div className="flex items-center gap-2 text-amber-400">
          <span>Edit Mode</span>
          {totalSelected > 0 && (
            <>
              <span className="text-cad-text-dim">|</span>
              <span>Selected:</span>
              <span className="font-mono">{formatPoints(totalSelected)}</span>
            </>
          )}
        </div>
      )}

      {/* Loaded pointclouds */}
      <div className="flex items-center gap-2">
        <span>Pointclouds:</span>
        <span className="text-cad-text font-mono">{pointclouds.length}</span>
      </div>

      {/* Total points */}
      <div className="flex items-center gap-2">
        <span>Total Points:</span>
        <span className="text-cad-text font-mono">{formatPoints(totalPoints)}</span>
      </div>

      {/* Point budget */}
      <div className="flex items-center gap-2">
        <span>Point Budget:</span>
        <span className="text-cad-text font-mono">{formatPoints(pointBudget)}</span>
      </div>

      {/* Spacer */}
      <div className="flex-1" />

      {/* Indexing status */}
      {pointclouds.some((pc) => pc.indexingProgress < 1.0) && (
        <div className="flex items-center gap-2 text-yellow-400">
          <span>Indexing...</span>
          {pointclouds
            .filter((pc) => pc.indexingProgress < 1.0)
            .map((pc) => (
              <span key={pc.id} className="font-mono">
                {pc.fileName}: {(pc.indexingProgress * 100).toFixed(0)}%
              </span>
            ))}
        </div>
      )}
    </div>
  );
});
