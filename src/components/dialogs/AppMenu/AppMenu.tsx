import { useState } from 'react';
import { ArrowLeft, FolderOpen, Download, Upload, Info } from 'lucide-react';
import { useAppStore } from '../../../state/appStore';
import { useRibbonActions } from '../../layout/Ribbon/useRibbonActions';

type AppMenuView = 'none' | 'import' | 'export' | 'about';

interface AppMenuProps {
  isOpen: boolean;
  onClose: () => void;
}

interface AppMenuItemProps {
  icon: React.ReactNode;
  label: string;
  shortcut?: string;
  onClick: () => void;
  active?: boolean;
  disabled?: boolean;
}

function AppMenuItem({ icon, label, shortcut, onClick, active, disabled }: AppMenuItemProps) {
  return (
    <button
      className={`w-full flex items-center gap-3 px-6 py-3 text-left text-sm transition-colors cursor-default ${
        disabled
          ? 'text-cad-text-muted opacity-50'
          : active
          ? 'bg-cad-hover text-cad-text'
          : 'text-cad-text hover:bg-cad-hover'
      }`}
      onClick={onClick}
      disabled={disabled}
    >
      <span className="w-5 h-5 flex items-center justify-center opacity-80">{icon}</span>
      <span className="flex-1">{label}</span>
      {shortcut && <span className="text-[10px] text-cad-text-muted font-mono">{shortcut}</span>}
    </button>
  );
}

export function AppMenu({ isOpen, onClose }: AppMenuProps) {
  const [activeView, setActiveView] = useState<AppMenuView>('none');
  const activePointcloudId = useAppStore((s) => s.activePointcloudId);
  const hasActivePointcloud = !!activePointcloudId;
  const actions = useRibbonActions();

  if (!isOpen) return null;

  const handleOpen = () => {
    onClose();
    actions.handleImport();
  };

  const handleExport = (format: string) => {
    onClose();
    actions.handleExport(format as 'ply-binary' | 'ply-ascii' | 'obj' | 'xyz' | 'pts' | 'csv');
  };

  return (
    <div className="fixed inset-0 z-50 flex bg-cad-bg">
      {/* Sidebar */}
      <div className="w-[280px] bg-cad-surface flex flex-col border-r border-cad-border">
        {/* Orange "File" close button */}
        <button
          className="flex items-center gap-3 px-5 py-4 text-white font-semibold text-base bg-cad-accent hover:bg-cad-accent/80 transition-colors cursor-default"
          onClick={onClose}
        >
          <ArrowLeft size={16} />
          File
        </button>

        <div className="flex flex-col py-2 flex-1">
          <AppMenuItem
            icon={<FolderOpen size={16} />}
            label="Open"
            shortcut="Ctrl+O"
            onClick={handleOpen}
          />

          <div className="h-px bg-cad-border my-1 mx-4" />

          <AppMenuItem
            icon={<Download size={16} />}
            label="Import"
            onClick={() => setActiveView('import')}
            active={activeView === 'import'}
          />
          <AppMenuItem
            icon={<Upload size={16} />}
            label="Export"
            onClick={() => setActiveView('export')}
            active={activeView === 'export'}
            disabled={!hasActivePointcloud}
          />

          <div className="flex-1" />

          <div className="h-px bg-cad-border my-1 mx-4" />

          <AppMenuItem
            icon={<Info size={16} />}
            label="About"
            onClick={() => setActiveView('about')}
            active={activeView === 'about'}
          />
        </div>
      </div>

      {/* Main content */}
      <div className="flex-1 overflow-y-auto">
        {activeView === 'none' && (
          <div className="p-8 text-cad-text-muted text-sm">
            Select an action from the sidebar.
          </div>
        )}

        {activeView === 'import' && (
          <div className="p-8">
            <h2 className="text-lg font-semibold text-cad-text mb-6">Import Pointcloud</h2>
            <div className="max-w-md">
              <button
                className="flex items-center gap-4 px-5 py-4 rounded bg-cad-surface border border-cad-border hover:border-cad-border-light hover:bg-cad-hover transition-colors cursor-default text-left w-full"
                onClick={handleOpen}
              >
                <Download size={24} className="text-cad-text-dim" />
                <div>
                  <div className="text-sm font-medium text-cad-text">Open pointcloud file</div>
                  <div className="text-xs text-cad-text-muted mt-0.5">
                    LAS, LAZ, E57, PLY, PCD, PTS, PTX, XYZ, ASC
                  </div>
                </div>
              </button>
            </div>
          </div>
        )}

        {activeView === 'export' && (
          <div className="p-8">
            <h2 className="text-lg font-semibold text-cad-text mb-6">Export Pointcloud</h2>
            <div className="flex flex-col gap-2 max-w-md">
              {[
                { format: 'ply-binary', label: 'PLY (Binary)', desc: 'Stanford Polygon (.ply)' },
                { format: 'ply-ascii', label: 'PLY (ASCII)', desc: 'Stanford Polygon, text (.ply)' },
                { format: 'obj', label: 'OBJ (Mesh)', desc: 'Wavefront OBJ (.obj)' },
                { format: 'xyz', label: 'XYZ', desc: 'Plain-text coordinates (.xyz)' },
                { format: 'pts', label: 'PTS', desc: 'Leica PTS format (.pts)' },
                { format: 'csv', label: 'CSV', desc: 'Comma-separated values (.csv)' },
              ].map(({ format, label, desc }) => (
                <button
                  key={format}
                  className="flex items-center gap-4 px-5 py-4 rounded bg-cad-surface border border-cad-border hover:border-cad-border-light hover:bg-cad-hover transition-colors cursor-default text-left"
                  onClick={() => handleExport(format)}
                >
                  <Upload size={20} className="text-cad-text-dim flex-shrink-0" />
                  <div>
                    <div className="text-sm font-medium text-cad-text">{label}</div>
                    <div className="text-xs text-cad-text-muted mt-0.5">{desc}</div>
                  </div>
                </button>
              ))}
            </div>
          </div>
        )}

        {activeView === 'about' && (
          <div className="p-8">
            <h2 className="text-lg font-semibold text-cad-text mb-6">About</h2>
            <div className="max-w-md">
              <h1 className="text-xl font-bold text-cad-text mb-1">Open Pointcloud Studio</h1>
              <p className="text-sm text-cad-text-dim mb-4">An open-source ReCap-equivalent for the AEC industry</p>
              <p className="text-sm text-cad-text-dim mb-4">
                Part of the OpenAEC Foundation.
              </p>
              <a
                href="https://github.com/OpenAEC-Foundation/open-pointcloud-studio"
                target="_blank"
                rel="noopener noreferrer"
                className="text-sm text-cad-accent hover:underline"
              >
                github.com/OpenAEC-Foundation/open-pointcloud-studio
              </a>
            </div>
          </div>
        )}
      </div>

      {/* Hidden file input for browser-mode open */}
      <input
        ref={actions.fileInputRef}
        type="file"
        accept={actions.FILE_INPUT_ACCEPT}
        multiple
        style={{ display: 'none' }}
        onChange={actions.handleFileInputChange}
      />
    </div>
  );
}
