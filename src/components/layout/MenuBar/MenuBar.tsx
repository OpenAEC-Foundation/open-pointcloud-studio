import { useState, useEffect, memo } from 'react';
import { Settings } from 'lucide-react';
import { getCurrentWindow } from '@tauri-apps/api/window';

// Detect platform
type Platform = 'windows' | 'linux' | 'macos';

function getPlatform(): Platform {
  const userAgent = navigator.userAgent.toLowerCase();
  if (userAgent.includes('win')) return 'windows';
  if (userAgent.includes('mac')) return 'macos';
  return 'linux';
}

// Windows-style window controls
function WindowsControls({
  onMinimize,
  onMaximize,
  onClose,
  isMaximized
}: {
  onMinimize: () => void;
  onMaximize: () => void;
  onClose: () => void;
  isMaximized: boolean;
}) {
  return (
    <div className="flex items-center h-full">
      <button
        onClick={onMinimize}
        className="w-[46px] h-full flex items-center justify-center hover:bg-[#3d3d3d] transition-colors cursor-default"
        title="Minimize"
      >
        <svg width="10" height="1" viewBox="0 0 10 1" fill="currentColor">
          <rect width="10" height="1" />
        </svg>
      </button>
      <button
        onClick={onMaximize}
        className="w-[46px] h-full flex items-center justify-center hover:bg-[#3d3d3d] transition-colors cursor-default"
        title={isMaximized ? 'Restore Down' : 'Maximize'}
      >
        {isMaximized ? (
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1">
            <rect x="2" y="0.5" width="7" height="7" />
            <polyline points="0.5,2.5 0.5,9.5 7.5,9.5 7.5,7.5" />
          </svg>
        ) : (
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1">
            <rect x="0.5" y="0.5" width="9" height="9" />
          </svg>
        )}
      </button>
      <button
        onClick={onClose}
        className="w-[46px] h-full flex items-center justify-center hover:bg-[#c42b1c] transition-colors group cursor-default"
        title="Close"
      >
        <svg width="10" height="10" viewBox="0 0 10 10" stroke="currentColor" strokeWidth="1.2" className="group-hover:stroke-white">
          <line x1="0" y1="0" x2="10" y2="10" />
          <line x1="10" y1="0" x2="0" y2="10" />
        </svg>
      </button>
    </div>
  );
}

// Linux/GNOME-style window controls
function LinuxControls({
  onMinimize,
  onMaximize,
  onClose,
  isMaximized
}: {
  onMinimize: () => void;
  onMaximize: () => void;
  onClose: () => void;
  isMaximized: boolean;
}) {
  return (
    <div className="flex items-center h-full gap-2 px-3">
      <button
        onClick={onMinimize}
        className="w-3 h-3 rounded-full bg-[#f5c211] hover:bg-[#d9a900] transition-colors flex items-center justify-center group cursor-default"
        title="Minimize"
      >
        <svg width="6" height="1" viewBox="0 0 6 1" className="opacity-0 group-hover:opacity-100 transition-opacity">
          <rect width="6" height="1" fill="#000" fillOpacity="0.6" />
        </svg>
      </button>
      <button
        onClick={onMaximize}
        className="w-3 h-3 rounded-full bg-[#2ecc71] hover:bg-[#27ae60] transition-colors flex items-center justify-center group cursor-default"
        title={isMaximized ? 'Restore' : 'Maximize'}
      >
        {isMaximized ? (
          <svg width="6" height="6" viewBox="0 0 6 6" className="opacity-0 group-hover:opacity-100 transition-opacity">
            <rect x="0.5" y="2" width="3" height="3" fill="none" stroke="#000" strokeOpacity="0.6" strokeWidth="1" />
            <polyline points="2,2 2,0.5 5.5,0.5 5.5,4 4,4" fill="none" stroke="#000" strokeOpacity="0.6" strokeWidth="1" />
          </svg>
        ) : (
          <svg width="6" height="6" viewBox="0 0 6 6" className="opacity-0 group-hover:opacity-100 transition-opacity">
            <rect x="0.5" y="0.5" width="5" height="5" fill="none" stroke="#000" strokeOpacity="0.6" strokeWidth="1" />
          </svg>
        )}
      </button>
      <button
        onClick={onClose}
        className="w-3 h-3 rounded-full bg-[#e95420] hover:bg-[#c44117] transition-colors flex items-center justify-center group cursor-default"
        title="Close"
      >
        <svg width="6" height="6" viewBox="0 0 6 6" className="opacity-0 group-hover:opacity-100 transition-opacity">
          <line x1="1" y1="1" x2="5" y2="5" stroke="#000" strokeOpacity="0.6" strokeWidth="1" />
          <line x1="5" y1="1" x2="1" y2="5" stroke="#000" strokeOpacity="0.6" strokeWidth="1" />
        </svg>
      </button>
    </div>
  );
}

function WindowControls() {
  const [isMaximized, setIsMaximized] = useState(false);
  const [platform] = useState<Platform>(getPlatform);

  const isTauri = !!(window as any).__TAURI_INTERNALS__;
  const appWindow = isTauri ? getCurrentWindow() : null;

  useEffect(() => {
    if (!appWindow) return;
    appWindow.isMaximized().then(setIsMaximized);

    const unlisten = appWindow.onResized(() => {
      appWindow.isMaximized().then(setIsMaximized);
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [appWindow]);

  if (!appWindow) return null;

  const handleMinimize = () => appWindow.minimize();
  const handleMaximize = () => appWindow.toggleMaximize();
  const handleClose = () => appWindow.close();

  const controlProps = {
    onMinimize: handleMinimize,
    onMaximize: handleMaximize,
    onClose: handleClose,
    isMaximized,
  };

  if (platform === 'linux') {
    return <LinuxControls {...controlProps} />;
  }

  return <WindowsControls {...controlProps} />;
}

export const MenuBar = memo(function MenuBar({ onSettingsClick }: { onSettingsClick: () => void }) {
  return (
    <div className="h-8 bg-cad-surface border-b border-cad-border flex items-center select-none">
      {/* Logo + Quick Access */}
      <div className="flex items-center gap-2 pr-3">
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" className="w-5 h-5 ml-3">
          <circle cx="24" cy="20" r="2.2" fill="#00b4d8"/><circle cx="24" cy="26" r="2.3" fill="#00b4d8"/><circle cx="24" cy="32" r="2.4" fill="#00b4d8"/><circle cx="24" cy="38" r="2.5" fill="#00b4d8"/><circle cx="24" cy="44" r="2.5" fill="#00b4d8"/><circle cx="24" cy="50" r="2.6" fill="#00b4d8"/><circle cx="24" cy="56" r="2.6" fill="#00b4d8"/><circle cx="24" cy="62" r="2.7" fill="#00b4d8"/><circle cx="24" cy="68" r="2.7" fill="#00b4d8"/><circle cx="24" cy="74" r="2.8" fill="#00b4d8"/><circle cx="24" cy="80" r="2.8" fill="#00b4d8"/>
          <circle cx="19" cy="20" r="1.5" fill="#0ea5e9" opacity="0.7"/><circle cx="29" cy="20" r="1.5" fill="#0ea5e9" opacity="0.7"/><circle cx="19" cy="80" r="1.8" fill="#0ea5e9" opacity="0.8"/><circle cx="29" cy="80" r="1.8" fill="#0ea5e9" opacity="0.8"/>
          <circle cx="76" cy="20" r="2.2" fill="#00b4d8"/><circle cx="76" cy="26" r="2.3" fill="#00b4d8"/><circle cx="76" cy="32" r="2.4" fill="#00b4d8"/><circle cx="76" cy="38" r="2.5" fill="#00b4d8"/><circle cx="76" cy="44" r="2.5" fill="#00b4d8"/><circle cx="76" cy="50" r="2.6" fill="#00b4d8"/><circle cx="76" cy="56" r="2.6" fill="#00b4d8"/><circle cx="76" cy="62" r="2.7" fill="#00b4d8"/><circle cx="76" cy="68" r="2.7" fill="#00b4d8"/><circle cx="76" cy="74" r="2.8" fill="#00b4d8"/><circle cx="76" cy="80" r="2.8" fill="#00b4d8"/>
          <circle cx="71" cy="20" r="1.5" fill="#0ea5e9" opacity="0.7"/><circle cx="81" cy="20" r="1.5" fill="#0ea5e9" opacity="0.7"/><circle cx="71" cy="80" r="1.8" fill="#0ea5e9" opacity="0.8"/><circle cx="81" cy="80" r="1.8" fill="#0ea5e9" opacity="0.8"/>
          <circle cx="30" cy="20" r="2" fill="#e94560"/><circle cx="36" cy="20" r="2.1" fill="#e94560"/><circle cx="42" cy="20" r="2.1" fill="#e94560"/><circle cx="50" cy="20" r="2.2" fill="#e94560"/><circle cx="58" cy="20" r="2.1" fill="#e94560"/><circle cx="64" cy="20" r="2.1" fill="#e94560"/><circle cx="70" cy="20" r="2" fill="#e94560"/>
          <circle cx="36" cy="17" r="1.2" fill="#fb7185" opacity="0.6"/><circle cx="50" cy="17" r="1.3" fill="#fb7185" opacity="0.65"/><circle cx="64" cy="17" r="1.2" fill="#fb7185" opacity="0.6"/><circle cx="36" cy="23" r="1.2" fill="#fb7185" opacity="0.6"/><circle cx="50" cy="23" r="1.3" fill="#fb7185" opacity="0.65"/><circle cx="64" cy="23" r="1.2" fill="#fb7185" opacity="0.6"/>
          <circle cx="28" cy="24" r="1.4" fill="#94a3b8" opacity="0.5"/><circle cx="33" cy="30" r="1.5" fill="#94a3b8" opacity="0.55"/><circle cx="38" cy="36" r="1.6" fill="#94a3b8" opacity="0.6"/><circle cx="43" cy="42" r="1.7" fill="#94a3b8" opacity="0.65"/><circle cx="48" cy="48" r="1.7" fill="#94a3b8" opacity="0.65"/>
          <circle cx="72" cy="24" r="1.4" fill="#94a3b8" opacity="0.5"/><circle cx="67" cy="30" r="1.5" fill="#94a3b8" opacity="0.55"/><circle cx="62" cy="36" r="1.6" fill="#94a3b8" opacity="0.6"/><circle cx="57" cy="42" r="1.7" fill="#94a3b8" opacity="0.65"/><circle cx="52" cy="48" r="1.7" fill="#94a3b8" opacity="0.65"/>
          <circle cx="30" cy="52" r="1.3" fill="#64748b" opacity="0.45"/><circle cx="38" cy="58" r="1.4" fill="#64748b" opacity="0.5"/><circle cx="46" cy="64" r="1.5" fill="#64748b" opacity="0.55"/><circle cx="54" cy="70" r="1.6" fill="#64748b" opacity="0.55"/><circle cx="62" cy="76" r="1.5" fill="#64748b" opacity="0.5"/>
          <circle cx="70" cy="52" r="1.3" fill="#64748b" opacity="0.45"/><circle cx="62" cy="58" r="1.4" fill="#64748b" opacity="0.5"/><circle cx="54" cy="64" r="1.5" fill="#64748b" opacity="0.55"/><circle cx="46" cy="70" r="1.6" fill="#64748b" opacity="0.55"/><circle cx="38" cy="76" r="1.5" fill="#64748b" opacity="0.5"/>
          <circle cx="30" cy="50" r="1.6" fill="#e94560" opacity="0.7"/><circle cx="37" cy="50" r="1.7" fill="#e94560" opacity="0.75"/><circle cx="44" cy="50" r="1.8" fill="#e94560" opacity="0.8"/><circle cx="50" cy="50" r="1.8" fill="#e94560" opacity="0.8"/><circle cx="56" cy="50" r="1.8" fill="#e94560" opacity="0.8"/><circle cx="63" cy="50" r="1.7" fill="#e94560" opacity="0.75"/><circle cx="70" cy="50" r="1.6" fill="#e94560" opacity="0.7"/>
          <circle cx="16" cy="83" r="1.8" fill="#475569" opacity="0.6"/><circle cx="24" cy="84" r="2" fill="#475569" opacity="0.65"/><circle cx="32" cy="83" r="1.6" fill="#475569" opacity="0.55"/><circle cx="68" cy="83" r="1.6" fill="#475569" opacity="0.55"/><circle cx="76" cy="84" r="2" fill="#475569" opacity="0.65"/><circle cx="84" cy="83" r="1.8" fill="#475569" opacity="0.6"/>
          <circle cx="40" cy="28" r="0.8" fill="#e94560" opacity="0.3"/><circle cx="60" cy="32" r="0.7" fill="#e94560" opacity="0.25"/><circle cx="50" cy="38" r="0.9" fill="#e94560" opacity="0.3"/><circle cx="34" cy="46" r="0.8" fill="#00b4d8" opacity="0.2"/><circle cx="66" cy="46" r="0.8" fill="#00b4d8" opacity="0.2"/><circle cx="50" cy="60" r="0.7" fill="#94a3b8" opacity="0.25"/>
        </svg>
        <div className="w-px h-4 bg-cad-border" />
        <button
          className="flex items-center justify-center w-6 h-6 hover:bg-cad-hover text-cad-text-dim hover:text-cad-text transition-colors cursor-default"
          title="Settings"
          onClick={onSettingsClick}
        >
          <Settings size={14} />
        </button>
      </div>

      {/* Draggable area with app title */}
      <div
        data-tauri-drag-region
        className="flex-1 h-full flex items-center justify-center cursor-default"
        onDoubleClick={() => {
          const isTauri = !!(window as any).__TAURI_INTERNALS__;
          if (isTauri) getCurrentWindow().toggleMaximize();
        }}
      >
        <span className="text-cad-text-dim text-sm font-medium pointer-events-none">
          Open Pointcloud Studio
        </span>
        <span className="text-cad-text-muted text-[10px] ml-1.5 pointer-events-none">
          v0.2.0
        </span>
      </div>

      {/* Window controls */}
      <WindowControls />
    </div>
  );
});
