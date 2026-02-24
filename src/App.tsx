import { useEffect, useRef, useState } from 'react';
import { PanelRightOpen, PanelRightClose } from 'lucide-react';
import { MenuBar } from './components/layout/MenuBar/MenuBar';
import { Ribbon } from './components/layout/Ribbon/Ribbon';
import { StatusBar } from './components/layout/StatusBar/StatusBar';
import { PointcloudViewer } from './components/canvas/PointcloudViewer';
import { PointcloudPanel } from './components/panels/PointcloudPanel';
import { useAppStore } from './state/appStore';

function App() {
  // Apply theme on mount
  const uiTheme = useAppStore((s) => s.uiTheme);
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', uiTheme);
  }, [uiTheme]);

  const rightPanelOpen = useAppStore((s) => s.rightPanelOpen);
  const toggleRightPanel = useAppStore((s) => s.toggleRightPanel);

  // Right panel resizing
  const [rightPanelWidth, setRightPanelWidth] = useState(256);
  const isResizingRight = useRef(false);

  useEffect(() => {
    const handleMouseMove = (e: MouseEvent) => {
      if (!isResizingRight.current) return;
      const newWidth = Math.max(180, Math.min(500, window.innerWidth - e.clientX));
      setRightPanelWidth(newWidth);
    };
    const handleMouseUp = () => {
      if (isResizingRight.current) {
        isResizingRight.current = false;
        document.body.style.cursor = '';
        document.body.style.userSelect = '';
      }
    };
    window.addEventListener('mousemove', handleMouseMove);
    window.addEventListener('mouseup', handleMouseUp);
    return () => {
      window.removeEventListener('mousemove', handleMouseMove);
      window.removeEventListener('mouseup', handleMouseUp);
    };
  }, []);

  // Block browser shortcuts in production
  useEffect(() => {
    if (import.meta.env.DEV) return;

    const handler = (e: KeyboardEvent) => {
      if (e.key === 'F5' || (e.ctrlKey && e.key === 'r')) e.preventDefault();
      if (e.ctrlKey && e.shiftKey && e.key === 'I') e.preventDefault();
      if (e.ctrlKey && e.key === 'u') e.preventDefault();
      if (e.key === 'F7') e.preventDefault();
    };

    const contextHandler = (e: MouseEvent) => e.preventDefault();

    window.addEventListener('keydown', handler, true);
    window.addEventListener('contextmenu', contextHandler, true);
    return () => {
      window.removeEventListener('keydown', handler, true);
      window.removeEventListener('contextmenu', contextHandler, true);
    };
  }, []);

  return (
    <div className="flex flex-col h-full w-full bg-cad-bg text-cad-text no-select">
      {/* Menu Bar */}
      <MenuBar />

      {/* Ribbon */}
      <Ribbon />

      {/* Main Content Area */}
      <div className="flex flex-1 overflow-hidden">
        {/* Center - Pointcloud Viewer */}
        <div className="flex-1 flex flex-col overflow-hidden relative">
          <div className="flex-1 relative overflow-hidden">
            <PointcloudViewer />
          </div>
        </div>

        {/* Right Panel - Pointcloud Properties */}
        {rightPanelOpen ? (
          <div
            className="bg-cad-surface border-l border-cad-border flex flex-col overflow-hidden relative"
            style={{ width: rightPanelWidth, minWidth: 180, maxWidth: 500 }}
          >
            {/* Resize handle */}
            <div
              className="absolute top-0 left-0 w-px h-full cursor-col-resize hover:bg-cad-accent z-10"
              onMouseDown={(e) => {
                e.preventDefault();
                isResizingRight.current = true;
                document.body.style.cursor = 'col-resize';
                document.body.style.userSelect = 'none';
              }}
            />
            {/* Panel header */}
            <div className="flex items-center justify-between px-3 h-7 min-h-[28px] select-none border-b border-cad-border bg-cad-surface">
              <span className="text-xs font-semibold text-cad-text">Pointcloud</span>
              <button
                type="button"
                onClick={toggleRightPanel}
                className="flex items-center justify-center w-5 h-5 rounded hover:bg-cad-hover text-cad-text-dim hover:text-cad-text transition-colors"
                title="Collapse right panel"
              >
                <PanelRightClose size={14} />
              </button>
            </div>
            <div className="flex-1 overflow-y-auto">
              <PointcloudPanel />
            </div>
          </div>
        ) : (
          <div className="flex flex-col bg-cad-surface border-l border-cad-border" style={{ width: 28 }}>
            <button
              type="button"
              onClick={toggleRightPanel}
              className="flex items-center justify-center w-full h-7 hover:bg-cad-hover text-cad-text-dim hover:text-cad-text transition-colors"
              title="Expand right panel"
            >
              <PanelRightOpen size={16} />
            </button>
          </div>
        )}
      </div>

      {/* Bottom Status Bar */}
      <StatusBar />
    </div>
  );
}

export default App;
