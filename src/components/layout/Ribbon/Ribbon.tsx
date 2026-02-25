import { useState, useRef, useEffect, memo, useCallback } from 'react';
import { Upload, Check, ChevronDown, Sun, Eye, BoxSelect, Trash2, XCircle, Move, Maximize, Filter, Building2, Shapes, Download } from 'lucide-react';
import { useAppStore } from '../../../state/appStore';
import { type UITheme, UI_THEMES } from '../../../state/appStore';
import { parsePointcloudFile, FILE_INPUT_ACCEPT, SUPPORTED_EXTENSIONS } from '../../../engine/pointcloud/PointcloudParser';
import { setBrowserPointcloud, getBrowserPointcloud } from '../../../engine/pointcloud/BrowserPointcloudStore';
import { reconstructSurface, type ReconstructionProgress } from '../../../engine/pointcloud/SurfaceReconstruction';
import { ReconstructionProgressDialog } from '../../panels/ReconstructionProgressDialog';
import { exportToOBJ, downloadOBJ } from '../../../engine/pointcloud/MeshExporter';
import { exportToPLY, exportToXYZ, exportToPTS, exportToCSV, downloadFile } from '../../../engine/pointcloud/PointcloudExporter';
import { translatePointcloud, scalePointcloud, thinPointcloud } from '../../../engine/pointcloud/PointcloudTransforms';
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

interface RibbonDropdownButtonProps {
  icon: React.ReactNode;
  label: string;
  disabled?: boolean;
  tooltip?: string;
  items: { label: string; onClick: () => void }[];
}

function RibbonDropdownButton({ icon, label, disabled, tooltip, items }: RibbonDropdownButtonProps) {
  const [isOpen, setIsOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);
  const tt = useTooltip();

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

  return (
    <div className="ribbon-dropdown-wrapper" ref={dropdownRef} style={{ position: 'relative' }}>
      <button
        ref={tt.ref}
        className={`ribbon-btn ${disabled ? 'disabled' : ''}`}
        onClick={() => !disabled && setIsOpen(!isOpen)}
        disabled={disabled}
        onMouseEnter={tt.onEnter}
        onMouseLeave={tt.onLeave}
      >
        <span className="ribbon-btn-icon">{icon}</span>
        <span className="ribbon-btn-label" style={{ display: 'flex', alignItems: 'center', gap: '2px' }}>
          {label} <ChevronDown size={10} />
        </span>
      </button>
      {tt.show && !isOpen && <RibbonTooltip label={tooltip || label} parentRef={tt.ref as React.RefObject<HTMLElement>} />}
      {isOpen && (
        <div
          className="ribbon-theme-menu"
          style={{
            position: 'absolute',
            top: '100%',
            left: 0,
            zIndex: 9999,
            minWidth: '120px',
          }}
        >
          {items.map((item) => (
            <button
              key={item.label}
              className="ribbon-theme-option"
              onClick={() => {
                item.onClick();
                setIsOpen(false);
              }}
            >
              <span>{item.label}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
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

type RibbonTab = 'home' | 'edit' | 'tools';

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
  const activePointcloudId = useAppStore((s) => s.activePointcloudId);
  const incrementTransformVersion = useAppStore((s) => s.incrementTransformVersion);
  const showBAG3DPanel = useAppStore((s) => s.showBAG3DPanel);
  const setShowBAG3DPanel = useAppStore((s) => s.setShowBAG3DPanel);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // Transform inputs
  const [translateX, setTranslateX] = useState(0);
  const [translateY, setTranslateY] = useState(0);
  const [translateZ, setTranslateZ] = useState(0);
  const [scaleX, setScaleX] = useState(1);
  const [scaleY, setScaleY] = useState(1);
  const [scaleZ, setScaleZ] = useState(1);
  const [thinPercent, setThinPercent] = useState(50);

  const totalSelected = Object.values(selectedPointIndices).reduce((sum, arr) => sum + arr.length, 0);

  const hasActivePointcloud = !!activePointcloudId;

  const handleTranslate = () => {
    if (!activePointcloudId) return;
    translatePointcloud(activePointcloudId, translateX, translateY, translateZ);
    incrementTransformVersion(activePointcloudId);
  };

  const handleScale = () => {
    if (!activePointcloudId) return;
    scalePointcloud(activePointcloudId, scaleX, scaleY, scaleZ);
    incrementTransformVersion(activePointcloudId);
  };

  const handleThin = () => {
    if (!activePointcloudId) return;
    const result = thinPointcloud(activePointcloudId, thinPercent);
    if (result) {
      const newCount = result.positions.length / 3;
      useAppStore.setState((s) => {
        const pc = s.pointclouds.find((p) => p.id === activePointcloudId);
        if (pc) pc.totalPoints = newCount;
      });
      incrementTransformVersion(activePointcloudId);
    }
  };

  const [reconstructing, setReconstructing] = useState(false);
  const [reconOpen, setReconOpen] = useState(false);
  const [reconPhase, setReconPhase] = useState('');
  const [reconPercent, setReconPercent] = useState(0);
  const reconCancelledRef = useRef({ value: false });

  const handleReconstruct = async () => {
    if (!activePointcloudId || reconstructing) return;
    const parsed = getBrowserPointcloud(activePointcloudId);
    if (!parsed) {
      alert('No pointcloud data found. Load a pointcloud first.');
      return;
    }

    reconCancelledRef.current = { value: false };
    setReconstructing(true);
    setReconOpen(true);
    setReconPhase('Initializing');
    setReconPercent(0);

    try {
      const result = await reconstructSurface(parsed.positions, {
        kNeighbors: 15,
        onProgress: (progress: ReconstructionProgress) => {
          setReconPhase(progress.phase);
          setReconPercent(progress.percent);
        },
        cancelled: reconCancelledRef.current,
      });

      // Update parsed data with mesh indices
      parsed.indices = result.indices;

      // Re-store and trigger viewer rebuild
      setBrowserPointcloud(activePointcloudId, parsed);
      incrementTransformVersion(activePointcloudId);

      // Brief "complete" state before closing
      setReconPhase(`Complete — ${result.indices.length / 3} triangles`);
      setReconPercent(100);
      await new Promise((r) => setTimeout(r, 1200));
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      if (msg !== 'Reconstruction cancelled') {
        alert(`Reconstruction failed:\n${msg}`);
      }
    } finally {
      setReconstructing(false);
      setReconOpen(false);
    }
  };

  const handleReconCancel = () => {
    reconCancelledRef.current.value = true;
    setReconPhase('Cancelling...');
  };

  const handleExportOBJ = () => {
    if (!activePointcloudId) return;
    const parsed = getBrowserPointcloud(activePointcloudId);
    if (!parsed) {
      alert('No pointcloud data found.');
      return;
    }
    if (!parsed.indices || parsed.indices.length === 0) {
      alert('No mesh data. Run "Reconstruct" first to generate a mesh.');
      return;
    }

    const pc = useAppStore.getState().pointclouds.find((p) => p.id === activePointcloudId);
    const baseName = pc?.fileName?.replace(/\.[^.]+$/, '') ?? 'mesh';

    const obj = exportToOBJ(parsed.positions, parsed.indices, undefined, parsed.colors);
    downloadOBJ(obj, `${baseName}.obj`);
  };

  const handleExport = (format: 'ply-binary' | 'ply-ascii' | 'obj' | 'xyz' | 'pts' | 'csv') => {
    if (!activePointcloudId) return;
    const parsed = getBrowserPointcloud(activePointcloudId);
    if (!parsed) {
      alert('No pointcloud data found. Load a pointcloud first.');
      return;
    }

    const pc = useAppStore.getState().pointclouds.find((p) => p.id === activePointcloudId);
    const baseName = pc?.fileName?.replace(/\.[^.]+$/, '') ?? 'pointcloud';

    switch (format) {
      case 'ply-binary': {
        const blob = exportToPLY(parsed, true);
        downloadFile(blob, `${baseName}.ply`);
        break;
      }
      case 'ply-ascii': {
        const blob = exportToPLY(parsed, false);
        downloadFile(blob, `${baseName}.ply`);
        break;
      }
      case 'obj': {
        if (!parsed.indices || parsed.indices.length === 0) {
          alert('No mesh data. Run "Reconstruct" first to export OBJ.');
          return;
        }
        const obj = exportToOBJ(parsed.positions, parsed.indices, undefined, parsed.colors);
        downloadOBJ(obj, `${baseName}.obj`);
        break;
      }
      case 'xyz': {
        const content = exportToXYZ(parsed);
        downloadFile(content, `${baseName}.xyz`);
        break;
      }
      case 'pts': {
        const content = exportToPTS(parsed);
        downloadFile(content, `${baseName}.pts`);
        break;
      }
      case 'csv': {
        const content = exportToCSV(parsed);
        downloadFile(content, `${baseName}.csv`);
        break;
      }
    }
  };

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
            transformVersion: 0,
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
        transformVersion: 0,
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
        <button
          className={`ribbon-tab ${activeTab === 'tools' ? 'active' : ''}`}
          onClick={() => setActiveTab('tools')}
        >
          Tools
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
              <RibbonDropdownButton
                icon={<Download size={20} />}
                label="Export"
                disabled={!hasActivePointcloud}
                tooltip="Export pointcloud to file"
                items={[
                  { label: 'PLY (Binary)', onClick: () => handleExport('ply-binary') },
                  { label: 'PLY (ASCII)', onClick: () => handleExport('ply-ascii') },
                  { label: 'OBJ (Mesh)', onClick: () => handleExport('obj') },
                  { label: 'XYZ', onClick: () => handleExport('xyz') },
                  { label: 'PTS', onClick: () => handleExport('pts') },
                  { label: 'CSV', onClick: () => handleExport('csv') },
                ]}
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

        {/* Tools tab */}
        <div className={`ribbon-content ${activeTab === 'tools' ? 'active' : ''}`}>
          <div className="ribbon-groups">
            {/* Translate Group */}
            <RibbonGroup label="Translate">
              <div className="ribbon-transform-inputs">
                <div className="ribbon-input-row">
                  <label>X</label>
                  <input type="number" className="ribbon-input" value={translateX} onChange={(e) => setTranslateX(Number(e.target.value))} step="1" />
                </div>
                <div className="ribbon-input-row">
                  <label>Y</label>
                  <input type="number" className="ribbon-input" value={translateY} onChange={(e) => setTranslateY(Number(e.target.value))} step="1" />
                </div>
                <div className="ribbon-input-row">
                  <label>Z</label>
                  <input type="number" className="ribbon-input" value={translateZ} onChange={(e) => setTranslateZ(Number(e.target.value))} step="1" />
                </div>
              </div>
              <RibbonButton
                icon={<Move size={20} />}
                label="Apply"
                onClick={handleTranslate}
                disabled={!hasActivePointcloud}
                tooltip="Translate pointcloud"
              />
            </RibbonGroup>

            {/* Scale Group */}
            <RibbonGroup label="Scale">
              <div className="ribbon-transform-inputs">
                <div className="ribbon-input-row">
                  <label>X</label>
                  <input type="number" className="ribbon-input" value={scaleX} onChange={(e) => setScaleX(Number(e.target.value))} step="0.1" />
                </div>
                <div className="ribbon-input-row">
                  <label>Y</label>
                  <input type="number" className="ribbon-input" value={scaleY} onChange={(e) => setScaleY(Number(e.target.value))} step="0.1" />
                </div>
                <div className="ribbon-input-row">
                  <label>Z</label>
                  <input type="number" className="ribbon-input" value={scaleZ} onChange={(e) => setScaleZ(Number(e.target.value))} step="0.1" />
                </div>
              </div>
              <RibbonButton
                icon={<Maximize size={20} />}
                label="Apply"
                onClick={handleScale}
                disabled={!hasActivePointcloud}
                tooltip="Scale pointcloud around centroid"
              />
            </RibbonGroup>

            {/* Thin Group */}
            <RibbonGroup label="Thin">
              <div className="ribbon-transform-inputs">
                <div className="ribbon-input-row">
                  <label>%</label>
                  <input
                    type="range"
                    min="1"
                    max="100"
                    value={thinPercent}
                    onChange={(e) => setThinPercent(Number(e.target.value))}
                    className="w-16"
                  />
                  <span className="ribbon-input-value">{thinPercent}%</span>
                </div>
              </div>
              <RibbonButton
                icon={<Filter size={20} />}
                label="Apply"
                onClick={handleThin}
                disabled={!hasActivePointcloud}
                tooltip={`Keep ${thinPercent}% of points`}
              />
            </RibbonGroup>

            {/* Surface Reconstruction Group */}
            <RibbonGroup label="Surface">
              <RibbonButton
                icon={<Shapes size={20} />}
                label={reconstructing ? 'Working...' : 'Reconstruct'}
                onClick={handleReconstruct}
                disabled={!hasActivePointcloud || reconstructing}
                tooltip="Reconstruct triangle mesh from pointcloud"
              />
              <RibbonButton
                icon={<Download size={20} />}
                label="Export OBJ"
                onClick={handleExportOBJ}
                disabled={!hasActivePointcloud}
                tooltip="Export mesh as Wavefront OBJ file"
              />
            </RibbonGroup>

            {/* 3D BAG Group */}
            <RibbonGroup label="Extensions">
              <RibbonButton
                icon={<Building2 size={20} />}
                label="3D BAG"
                onClick={() => setShowBAG3DPanel(!showBAG3DPanel)}
                active={showBAG3DPanel}
                tooltip="Download 3D buildings from 3dbag.nl"
              />
            </RibbonGroup>
          </div>
        </div>
      </div>
    </div>
    <ReconstructionProgressDialog
      open={reconOpen}
      phase={reconPhase}
      percent={reconPercent}
      onCancel={handleReconCancel}
    />
    </>
  );
});
