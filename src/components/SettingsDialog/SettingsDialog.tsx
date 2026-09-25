import { useState, useRef, useEffect } from 'react';
import './SettingsDialog.css';

interface SettingsDialogProps {
  onClose: () => void;
}

export function SettingsDialog({ onClose }: SettingsDialogProps) {
  // Dragging state
  const dialogRef = useRef<HTMLDivElement>(null);
  const [offset, setOffset] = useState<{ x: number; y: number } | null>(null);
  const dragging = useRef(false);
  const dragStart = useRef({ x: 0, y: 0 });

  useEffect(() => {
    const handleMouseMove = (e: MouseEvent) => {
      if (!dragging.current) return;
      setOffset({
        x: e.clientX - dragStart.current.x,
        y: e.clientY - dragStart.current.y,
      });
    };
    const handleMouseUp = () => {
      dragging.current = false;
    };
    window.addEventListener('mousemove', handleMouseMove);
    window.addEventListener('mouseup', handleMouseUp);
    return () => {
      window.removeEventListener('mousemove', handleMouseMove);
      window.removeEventListener('mouseup', handleMouseUp);
    };
  }, []);

  const handleHeaderMouseDown = (e: React.MouseEvent) => {
    if ((e.target as HTMLElement).closest('.settings-close')) return;
    dragging.current = true;
    dragStart.current = {
      x: e.clientX - (offset?.x ?? 0),
      y: e.clientY - (offset?.y ?? 0),
    };
  };

  return (
    <div className="settings-dialog-overlay">
      <div
        ref={dialogRef}
        className="settings-dialog"
        style={offset ? { transform: `translate(${offset.x}px, ${offset.y}px)` } : undefined}
      >
        {/* Header */}
        <div className="settings-header" onMouseDown={handleHeaderMouseDown}>
          <h2 className="settings-title">Settings</h2>
          <button className="settings-close" onClick={onClose}>&times;</button>
        </div>

        {/* Body: sidebar + content */}
        <div className="settings-body">
          <div className="settings-sidebar">
            <button className="settings-tab active">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <circle cx="12" cy="12" r="3" />
                <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
              </svg>
              <span>General</span>
            </button>
          </div>

          <div className="settings-content">
          </div>
        </div>

        {/* Footer */}
        <div className="settings-footer">
          <button className="settings-footer-close" onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  );
}
