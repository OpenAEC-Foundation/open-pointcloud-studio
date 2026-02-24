import { useState, useRef, useEffect, memo, useCallback } from 'react';
import { Upload, Check, ChevronDown, Sun, Eye, BoxSelect, Trash2, XCircle } from 'lucide-react';
import { useAppStore } from '../../../state/appStore';
import { type UITheme, UI_THEMES } from '../../../state/appStore';
import { parsePointcloudFile, FILE_INPUT_ACCEPT, SUPPORTED_EXTENSIONS } from '../../../engine/pointcloud/PointcloudParser';
import { setBrowserPointcloud } from '../../../engine/pointcloud/BrowserPointcloudStore';
import './Ribbon.css';

// ============================================================================
// Reusable Ribbon Components
// ============================================================================

function RibbonTooltip({ label, shortcut, parentRef }: { label: string; shortcut?: string; parentRef: React.RefObject<HTMLElement> }) {
  const [pos, setPos] = useState<{ x: number; y: number; align: 'center' | 'left' | 'right' } | null>(null);
  const tooltipRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (parentRef.current) {
      const rect = parentRef.current.getBoundingClientRect();
      const centerX = rect.left + rect.width / 2;
      const viewportWidth = window.innerWidth;
      const estimatedTooltipWidth = 150;
      const margin = 8;

      let align: 'center' | 'left' | 'right' = 'center';
      let x = centerX;

      if (centerX - estimatedTooltipWidth / 2 < margin) {
        align = 'left';
        x = margin;
      } else if (centerX + estimatedTooltipWidth / 2 > viewportWidth - margin) {
        align = 'right';
        x = viewportWidth - margin;
      }

      setPos({ x, y: rect.bottom + 4, align });
    }
  }, [parentRef]);

  useEffect(() => {
    if (tooltipRef.current && pos) {
      const tooltipRect = tooltipRef.current.getBoundingClientRect();
      const viewportWidth = window.innerWidth;
      const margin = 8;

      if (pos.align === 'center') {
        if (tooltipRect.left < margin) {
          setPos({ ...pos, x: margin, align: 'left' });
        } else if (tooltipRect.right > viewportWidth - margin) {
          setPos({ ...pos, x: viewportWidth - margin, align: 'right' });
        }
      }
    }
  }, [pos]);

  if (!pos) return null;

  const transformStyle = pos.align === 'center'
    ? 'translateX(-50%)'
    : pos.align === 'right'
      ? 'translateX(-100%)'
      : 'none';

  return (
    <div
      ref={tooltipRef}
      style={{
        position: 'fixed',
        left: pos.x,
        top: pos.y,
        transform: transformStyle,
        zIndex: 9999,
        pointerEvents: 'none',
      }}
    >
      <div className="ribbon-tooltip">
        <span className="ribbon-tooltip-label">{label}</span>
        {shortcut && <span className="ribbon-tooltip-shortcut">{shortcut}</span>}
      </div>
    </div>
  );
}

function useTooltip(delay = 400) {
  const [show, setShow] = useState(false);
  const ref = useRef<HTMLButtonElement>(null);
  const timerRef = useRef<ReturnType<typeof setTimeout>>();

  const onEnter = useCallback(() => {
    timerRef.current = setTimeout(() => setShow(true), delay);
  }, [delay]);

  const onLeave = useCallback(() => {
    if (timerRef.current) clearTimeout(timerRef.current);
    setShow(false);
  }, []);

  useEffect(() => {
    return () => { if (timerRef.current) clearTimeout(timerRef.current); };
  }, []);

  return { show, ref, onEnter, onLeave };
}

interface RibbonButtonProps {
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
  active?: boolean;
  disabled?: boolean;
  shortcut?: string;
  tooltip?: string;
}

function RibbonButton({ icon, label, onClick, active, disabled, shortcut, tooltip }: RibbonButtonProps) {
  const tt = useTooltip();
  return (
    <>
      <button
        ref={tt.ref}
        className={`ribbon-btn ${active ? 'active' : ''} ${disabled ? 'disabled' : ''}`}
        onClick={onClick}
        disabled={disabled}
        onMouseEnter={tt.onEnter}
        onMouseLeave={tt.onLeave}
      >
        <span className="ribbon-btn-icon">{icon}</span>
        <span className="ribbon-btn-label">{label}</span>
      </button>
      {tt.show && <RibbonTooltip label={tooltip || label} shortcut={shortcut} parentRef={tt.ref as React.RefObject<HTMLElement>} />}
    </>
  );
}

interface RibbonSmallButtonProps {
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
  active?: boolean;
  disabled?: boolean;
  shortcut?: string;
}

function RibbonSmallButton({ icon, label, onClick, active, disabled, shortcut }: RibbonSmallButtonProps) {
  const tt = useTooltip();
  return (
    <>
      <button
        ref={tt.ref}
        className={`ribbon-btn small ${active ? 'active' : ''} ${disabled ? 'disabled' : ''}`}
        onClick={onClick}
        disabled={disabled}
        onMouseEnter={tt.onEnter}
        onMouseLeave={tt.onLeave}
      >
        <span className="ribbon-btn-icon">{icon}</span>
        <span className="ribbon-btn-label">{label}</span>
      </button>
      {tt.show && <RibbonTooltip label={label} shortcut={shortcut} parentRef={tt.ref as React.RefObject<HTMLElement>} />}
    </>
  );
}

function RibbonGroup({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="ribbon-group">
      <div className="ribbon-group-content">{children}</div>
      <div className="ribbon-group-label">{label}</div>
    </div>
  );
}

function RibbonButtonStack({ children }: { children: React.ReactNode }) {
  return <div className="ribbon-btn-stack">{children}</div>;
}

// ============================================================================
// Theme Selector
// ============================================================================

interface ThemeSelectorProps {
  currentTheme: UITheme;
  onThemeChange: (theme: UITheme) => void;
}

function ThemeSelector({ currentTheme, onThemeChange }: ThemeSelectorProps) {
  const [isOpen, setIsOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setIsOpen(false);
      }
    };

    if (isOpen) {
      document.addEventListener('mousedown', handleClickOutside);
    }
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, [isOpen]);

  const currentThemeLabel = UI_THEMES.find(t => t.id === currentTheme)?.label || 'Dark';

  return (
    <div className="ribbon-theme-selector" ref={dropdownRef}>
      <span className="ribbon-theme-label">Theme</span>
      <div className="ribbon-theme-dropdown">
        <button
          className="ribbon-theme-button"
          onClick={() => setIsOpen(!isOpen)}
        >
          <span className="ribbon-theme-button-content">
            <span className={`ribbon-theme-swatch ${currentTheme}`} />
            <span>{currentThemeLabel}</span>
          </span>
          <ChevronDown size={12} />
        </button>
        {isOpen && (
          <div className="ribbon-theme-menu">
            {UI_THEMES.map((theme) => (
              <button
                key={theme.id}
                className={`ribbon-theme-option ${currentTheme === theme.id ? 'selected' : ''}`}
                onClick={() => {
                  onThemeChange(theme.id);
                  setIsOpen(false);
                }}
              >
                {currentTheme === theme.id ? (
                  <Check size={12} className="checkmark" />
                ) : (
                  <span className="no-check" />
                )}
                <span className={`ribbon-theme-swatch ${theme.id}`} />
                <span>{theme.label}</span>
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

// ============================================================================
// Color Mode Icons
// ============================================================================

function RGBIcon() {
  return (
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="5" />
      <circle cx="10" cy="11" r="1.5" fill="#ff4444" stroke="none" />
      <circle cx="14" cy="11" r="1.5" fill="#44ff44" stroke="none" />
      <circle cx="12" cy="14" r="1.5" fill="#4444ff" stroke="none" />
    </svg>
  );
}

function ElevationIcon() {
  return (
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M2 20l7-7 4 4 9-11" />
    </svg>
  );
}

function ClassificationIcon() {
  return (
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3" y="3" width="7" height="7" rx="1" />
      <rect x="14" y="3" width="7" height="7" rx="1" />
      <rect x="3" y="14" width="7" height="7" rx="1" />
      <rect x="14" y="14" width="7" height="7" rx="1" />
    </svg>
  );
}

function IntensityIcon() {
  return (
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="9" />
      <path d="M12 3v18" />
      <path d="M12 3a9 9 0 0 0 0 18" fill="currentColor" fillOpacity="0.3" />
    </svg>
  );
}

// ============================================================================
// Main Ribbon Component
// ============================================================================

type RibbonTab = 'home' | 'edit';

export const Ribbon = memo(function Ribbon() {
  const [activeTab, setActiveTab] = useState<RibbonTab>('home');
  const colorMode = useAppStore((s) => s.pointcloudColorMode);
  const setColorMode = useAppStore((s) => s.setPointcloudColorMode);
  const pointSize = useAppStore((s) => s.pointcloudPointSize);
  const setPointSize = useAppStore((s) => s.setPointcloudPointSize);
  const pointBudget = useAppStore((s) => s.pointBudget);
  const setPointBudget = useAppStore((s) => s.setPointBudget);
  const edlEnabled = useAppStore((s) => s.edlEnabled);
  const setEdlEnabled = useAppStore((s) => s.setEdlEnabled);
  const uiTheme = useAppStore((s) => s.uiTheme);
  const setUITheme = useAppStore((s) => s.setUITheme);
  const editMode = useAppStore((s) => s.editMode);
  const setEditMode = useAppStore((s) => s.setEditMode);
  const clearSelection = useAppStore((s) => s.clearSelection);
  const selectedPointIndices = useAppStore((s) => s.selectedPointIndices);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const totalSelected = Object.values(selectedPointIndices).reduce((sum, arr) => sum + arr.length, 0);

  const isTauri = !!(window as any).__TAURI_INTERNALS__;

  const handleImport = async () => {
    if (isTauri) {
      // Tauri mode: use native file dialog
      try {
        const { open } = await import('@tauri-apps/plugin-dialog');
        const result = await open({
          multiple: true,
          filters: [{ name: 'Point Clouds', extensions: SUPPORTED_EXTENSIONS.map(e => e.slice(1)) }],
        });
        if (!result) return;

        const files = Array.isArray(result) ? result : [result];
        const { invoke } = await import('@tauri-apps/api/core');

        for (const filePath of files) {
          const id = crypto.randomUUID();
          const fileName = filePath.split(/[/\\]/).pop() || 'unknown';

          useAppStore.getState().addPointcloud({
            id,
            fileName,
            filePath,
            format: filePath.endsWith('.laz') ? 'LAZ' : 'LAS',
            totalPoints: 0,
            bounds: { minX: 0, minY: 0, minZ: 0, maxX: 0, maxY: 0, maxZ: 0 },
            hasColor: false,
            hasIntensity: false,
            hasClassification: false,
            visible: true,
            indexingProgress: 0,
            indexingPhase: 'Opening...',
          });

          invoke('pointcloud_open', { id, path: filePath }).then((meta: any) => {
            useAppStore.setState((s) => {
              const pc = s.pointclouds.find((p) => p.id === id);
              if (pc) {
                pc.totalPoints = meta.total_points;
                pc.bounds = meta.bounds;
                pc.hasColor = meta.has_color;
                pc.hasIntensity = meta.has_intensity;
                pc.hasClassification = meta.has_classification;
                pc.indexingProgress = 1.0;
                pc.indexingPhase = 'Ready';
              }
            });
          }).catch((err: any) => {
            console.error('Failed to open pointcloud:', err);
            useAppStore.getState().removePointcloud(id);
          });
        }
      } catch (err) {
        console.error('Import failed:', err);
      }
    } else {
      // Browser mode: use HTML file input
      fileInputRef.current?.click();
    }
  };

  const handleFileInputChange = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const files = e.target.files;
    if (!files) return;

    for (const file of Array.from(files)) {
      const ext = file.name.substring(file.name.lastIndexOf('.')).toLowerCase();
      const format = ext.replace('.', '').toUpperCase();

      const id = crypto.randomUUID();
      useAppStore.getState().addPointcloud({
        id,
        fileName: file.name,
        filePath: file.name,
        format,
        totalPoints: 0,
        bounds: { minX: 0, minY: 0, minZ: 0, maxX: 0, maxY: 0, maxZ: 0 },
        hasColor: false,
        hasIntensity: false,
        hasClassification: false,
        visible: true,
        indexingProgress: 0,
        indexingPhase: 'Reading file...',
      });

      try {
        useAppStore.getState().updatePointcloudProgress(id, 0.3, 'Parsing...');

        const parsed = await parsePointcloudFile(file);
        const h = parsed.header;

        // Store parsed data for the viewer
        setBrowserPointcloud(id, parsed);

        // Update store with metadata
        useAppStore.setState((s) => {
          const pc = s.pointclouds.find((p) => p.id === id);
          if (pc) {
            pc.totalPoints = parsed.positions.length / 3;
            pc.bounds = {
              minX: h.minX, minY: h.minY, minZ: h.minZ,
              maxX: h.maxX, maxY: h.maxY, maxZ: h.maxZ,
            };
            pc.hasColor = parsed.hasColor;
            pc.hasIntensity = parsed.hasIntensity;
            pc.hasClassification = parsed.hasClassification;
            pc.indexingProgress = 1.0;
            pc.indexingPhase = 'Ready';
          }
        });
      } catch (err) {
        console.error('Failed to parse pointcloud file:', err);
        useAppStore.getState().removePointcloud(id);
        alert(`Failed to parse ${file.name}:\n${err instanceof Error ? err.message : String(err)}`);
      }
    }

    // Reset input so same file can be selected again
    e.target.value = '';
  };

  const formatBudget = (n: number) => {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 1_000) return `${(n / 1_000).toFixed(0)}K`;
    return n.toString();
  };

  return (
    <>
    {/* Hidden file input for browser mode */}
    <input
      ref={fileInputRef}
      type="file"
      accept={FILE_INPUT_ACCEPT}
      multiple
      style={{ display: 'none' }}
      onChange={handleFileInputChange}
    />
    <div className="ribbon-container">
      {/* Tab bar */}
      <div className="ribbon-tabs">
        <button
          className={`ribbon-tab ${activeTab === 'home' ? 'active' : ''}`}
          onClick={() => setActiveTab('home')}
        >
          Home
        </button>
        <button
          className={`ribbon-tab ${activeTab === 'edit' ? 'active' : ''}`}
          onClick={() => setActiveTab('edit')}
        >
          Edit
        </button>
      </div>

      {/* Content area */}
      <div className="ribbon-content-container">
        {/* Home tab */}
        <div className={`ribbon-content ${activeTab === 'home' ? 'active' : ''}`}>
          <div className="ribbon-groups">
            {/* File Group */}
            <RibbonGroup label="File">
              <RibbonButton
                icon={<Upload size={20} />}
                label="Import"
                onClick={handleImport}
                tooltip="Import pointcloud (.las/.laz)"
              />
            </RibbonGroup>

            {/* Color Group */}
            <RibbonGroup label="Color">
              <RibbonButtonStack>
                <RibbonSmallButton
                  icon={<RGBIcon />}
                  label="RGB"
                  onClick={() => setColorMode('rgb')}
                  active={colorMode === 'rgb'}
                />
                <RibbonSmallButton
                  icon={<ElevationIcon />}
                  label="Elevation"
                  onClick={() => setColorMode('elevation')}
                  active={colorMode === 'elevation'}
                />
              </RibbonButtonStack>
              <RibbonButtonStack>
                <RibbonSmallButton
                  icon={<ClassificationIcon />}
                  label="Class"
                  onClick={() => setColorMode('classification')}
                  active={colorMode === 'classification'}
                />
                <RibbonSmallButton
                  icon={<IntensityIcon />}
                  label="Intensity"
                  onClick={() => setColorMode('intensity')}
                  active={colorMode === 'intensity'}
                />
              </RibbonButtonStack>
            </RibbonGroup>

            {/* Display Group */}
            <RibbonGroup label="Display">
              <div className="flex flex-col gap-1 px-2 py-1 justify-center">
                <label className="flex items-center gap-1 text-[10px] text-cad-text-dim">
                  <span className="w-10">Size</span>
                  <input
                    type="range"
                    min="0.1"
                    max="20"
                    step="0.1"
                    value={pointSize}
                    onChange={(e) => setPointSize(Number(e.target.value))}
                    className="w-20"
                  />
                  <span className="w-4 text-cad-text text-right">{pointSize}</span>
                </label>
                <label className="flex items-center gap-1 text-[10px] text-cad-text-dim">
                  <span className="w-10">Budget</span>
                  <input
                    type="range"
                    min="100000"
                    max="10000000"
                    step="100000"
                    value={pointBudget}
                    onChange={(e) => setPointBudget(Number(e.target.value))}
                    className="w-20"
                  />
                  <span className="w-8 text-cad-text text-right">{formatBudget(pointBudget)}</span>
                </label>
              </div>
            </RibbonGroup>

            {/* Settings Group */}
            <RibbonGroup label="Settings">
              <RibbonButtonStack>
                <RibbonSmallButton
                  icon={<Eye size={14} />}
                  label="EDL"
                  onClick={() => setEdlEnabled(!edlEnabled)}
                  active={edlEnabled}
                />
                <RibbonSmallButton
                  icon={<Sun size={14} />}
                  label="Theme"
                  onClick={() => {
                    const themes: UITheme[] = ['dark', 'light', 'blue', 'highContrast'];
                    const idx = themes.indexOf(uiTheme);
                    setUITheme(themes[(idx + 1) % themes.length]);
                  }}
                />
              </RibbonButtonStack>
              <ThemeSelector currentTheme={uiTheme} onThemeChange={setUITheme} />
            </RibbonGroup>
          </div>
        </div>

        {/* Edit tab */}
        <div className={`ribbon-content ${activeTab === 'edit' ? 'active' : ''}`}>
          <div className="ribbon-groups">
            {/* Select Group */}
            <RibbonGroup label="Select">
              <RibbonButton
                icon={<BoxSelect size={20} />}
                label="Box Select"
                onClick={() => setEditMode(!editMode)}
                active={editMode}
                tooltip="Toggle box selection mode (Esc to exit)"
                shortcut="Esc"
              />
            </RibbonGroup>

            {/* Modify Group */}
            <RibbonGroup label="Modify">
              <RibbonButtonStack>
                <RibbonSmallButton
                  icon={<Trash2 size={14} />}
                  label="Delete"
                  onClick={() => {
                    // Dispatch delete via keyboard event simulation
                    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Delete' }));
                  }}
                  disabled={totalSelected === 0}
                  shortcut="Del"
                />
                <RibbonSmallButton
                  icon={<XCircle size={14} />}
                  label="Deselect"
                  onClick={() => clearSelection()}
                  disabled={totalSelected === 0}
                />
              </RibbonButtonStack>
            </RibbonGroup>

            {/* Selection info */}
            {editMode && totalSelected > 0 && (
              <RibbonGroup label="Info">
                <div className="flex items-center px-3 py-1 text-xs text-cad-text">
                  <span className="font-mono text-amber-400">{totalSelected.toLocaleString()}</span>
                  <span className="ml-1 text-cad-text-dim">points selected</span>
                </div>
              </RibbonGroup>
            )}
          </div>
        </div>
      </div>
    </div>
    </>
  );
});
