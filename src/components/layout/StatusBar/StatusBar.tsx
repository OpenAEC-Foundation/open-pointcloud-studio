import { memo } from 'react';
import { useAppStore } from '../../../state/appStore';

function formatPoints(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(0)}K`;
  return n.toString();
}

export const StatusBar = memo(function StatusBar() {
  const pointclouds = useAppStore((s) => s.pointclouds);
  const pointBudget = useAppStore((s) => s.pointBudget);

  const totalPoints = pointclouds.reduce((sum, pc) => sum + pc.totalPoints, 0);

  return (
    <div className="h-6 bg-cad-surface border-t border-cad-border flex items-center px-3 text-xs text-cad-text-dim gap-6">
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
