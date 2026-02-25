import { useEffect, useState } from 'react';
import { XCircle } from 'lucide-react';

interface ReconstructionProgressDialogProps {
  open: boolean;
  phase: string;
  percent: number;
  onCancel: () => void;
}

export function ReconstructionProgressDialog({
  open,
  phase,
  percent,
  onCancel,
}: ReconstructionProgressDialogProps) {
  const [startTime] = useState(() => Date.now());
  const [elapsed, setElapsed] = useState(0);

  useEffect(() => {
    if (!open) return;
    const interval = setInterval(() => {
      setElapsed(Date.now() - startTime);
    }, 200);
    return () => clearInterval(interval);
  }, [open, startTime]);

  if (!open) return null;

  const seconds = Math.floor(elapsed / 1000);
  const mins = Math.floor(seconds / 60);
  const secs = seconds % 60;
  const timeStr = mins > 0 ? `${mins}m ${secs}s` : `${secs}s`;
  const clampedPercent = Math.max(0, Math.min(100, Math.round(percent)));

  return (
    <div className="recon-progress-overlay">
      <div className="recon-progress-panel">
        <div className="recon-progress-header">
          <span className="recon-progress-title">Surface Reconstruction</span>
          <button className="recon-progress-close-btn" onClick={onCancel} title="Cancel">
            <XCircle size={16} />
          </button>
        </div>

        <div className="recon-progress-body">
          <div className="recon-progress-phase">{phase}</div>

          <div className="recon-progress-bar-track">
            <div
              className="recon-progress-bar-fill"
              style={{ width: `${clampedPercent}%` }}
            />
          </div>

          <div className="recon-progress-info">
            <span>{clampedPercent}%</span>
            <span>Elapsed: {timeStr}</span>
          </div>
        </div>

        <div className="recon-progress-footer">
          <button className="recon-progress-cancel-btn" onClick={onCancel}>
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
