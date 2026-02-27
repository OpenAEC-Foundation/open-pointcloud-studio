import { useState, memo } from 'react';
import { Upload, Sun, Eye, BoxSelect, Trash2, XCircle, Move, Maximize, Filter, Building2, Shapes, Download, Focus } from 'lucide-react';
import { useAppStore } from '../../../state/appStore';
import { type UITheme } from '../../../state/appStore';
import { RibbonButton, RibbonSmallButton, RibbonGroup, RibbonButtonStack, RibbonDropdownButton, ThemeSelector } from './RibbonComponents';
import { RGBIcon, ElevationIcon, ClassificationIcon, IntensityIcon } from './RibbonIcons';
import { useRibbonActions } from './useRibbonActions';
import { zoomToFit } from '../../canvas/PointcloudViewer';
import { ReconstructionProgressDialog } from '../../panels/ReconstructionProgressDialog';
import './Ribbon.css';

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
  const showBAG3DPanel = useAppStore((s) => s.showBAG3DPanel);
  const setShowBAG3DPanel = useAppStore((s) => s.setShowBAG3DPanel);

  const totalSelected = Object.values(selectedPointIndices).reduce((sum, arr) => sum + arr.length, 0);
  const hasActivePointcloud = !!activePointcloudId;

  const actions = useRibbonActions();

  return (
    <>
    {/* Hidden file input for browser mode */}
    <input
      ref={actions.fileInputRef}
      type="file"
      accept={actions.FILE_INPUT_ACCEPT}
      multiple
      style={{ display: 'none' }}
      onChange={actions.handleFileInputChange}
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
            <RibbonGroup label="File">
              <RibbonButton
                icon={<Download size={20} />}
                label="Import"
                onClick={actions.handleImport}
                tooltip="Import pointcloud (.las/.laz)"
              />
              <RibbonDropdownButton
                icon={<Upload size={20} />}
                label="Export"
                disabled={!hasActivePointcloud}
                tooltip="Export pointcloud to file"
                items={[
                  { label: 'PLY (Binary)', onClick: () => actions.handleExport('ply-binary') },
                  { label: 'PLY (ASCII)', onClick: () => actions.handleExport('ply-ascii') },
                  { label: 'OBJ (Mesh)', onClick: () => actions.handleExport('obj') },
                  { label: 'XYZ', onClick: () => actions.handleExport('xyz') },
                  { label: 'PTS', onClick: () => actions.handleExport('pts') },
                  { label: 'CSV', onClick: () => actions.handleExport('csv') },
                ]}
              />
            </RibbonGroup>

            <RibbonGroup label="Color">
              <RibbonButtonStack>
                <RibbonSmallButton icon={<RGBIcon />} label="RGB" onClick={() => setColorMode('rgb')} active={colorMode === 'rgb'} />
                <RibbonSmallButton icon={<ElevationIcon />} label="Elevation" onClick={() => setColorMode('elevation')} active={colorMode === 'elevation'} />
              </RibbonButtonStack>
              <RibbonButtonStack>
                <RibbonSmallButton icon={<ClassificationIcon />} label="Class" onClick={() => setColorMode('classification')} active={colorMode === 'classification'} />
                <RibbonSmallButton icon={<IntensityIcon />} label="Intensity" onClick={() => setColorMode('intensity')} active={colorMode === 'intensity'} />
              </RibbonButtonStack>
            </RibbonGroup>

            <RibbonGroup label="Display">
              <div className="ribbon-slider-group">
                <label className="ribbon-slider-row">
                  <span className="ribbon-slider-label">Size</span>
                  <input type="range" min="0.1" max="20" step="0.1" value={pointSize} onChange={(e) => setPointSize(Number(e.target.value))} className="ribbon-slider" />
                  <span className="ribbon-slider-value">{pointSize}</span>
                </label>
                <label className="ribbon-slider-row">
                  <span className="ribbon-slider-label">Budget</span>
                  <input type="range" min="100000" max="10000000" step="100000" value={pointBudget} onChange={(e) => setPointBudget(Number(e.target.value))} className="ribbon-slider" />
                  <span className="ribbon-slider-value">{actions.formatBudget(pointBudget)}</span>
                </label>
              </div>
            </RibbonGroup>

            <RibbonGroup label="View">
              <RibbonButton
                icon={<Focus size={20} />}
                label="Zoom All"
                onClick={zoomToFit}
                tooltip="Zoom to fit all geometry (F)"
              />
            </RibbonGroup>

            <RibbonGroup label="Settings">
              <RibbonButtonStack>
                <RibbonSmallButton icon={<Eye size={14} />} label="EDL" onClick={() => setEdlEnabled(!edlEnabled)} active={edlEnabled} />
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
            <RibbonGroup label="Select">
              <RibbonButton icon={<BoxSelect size={20} />} label="Box Select" onClick={() => setEditMode(!editMode)} active={editMode} tooltip="Toggle box selection mode (Esc to exit)" shortcut="Esc" />
            </RibbonGroup>

            <RibbonGroup label="Modify">
              <RibbonButtonStack>
                <RibbonSmallButton
                  icon={<Trash2 size={14} />}
                  label="Delete"
                  onClick={() => window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Delete' }))}
                  disabled={totalSelected === 0}
                  shortcut="Del"
                />
                <RibbonSmallButton icon={<XCircle size={14} />} label="Deselect" onClick={() => clearSelection()} disabled={totalSelected === 0} />
              </RibbonButtonStack>
            </RibbonGroup>

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
            <RibbonGroup label="Translate">
              <div className="ribbon-transform-inputs">
                <div className="ribbon-input-row">
                  <label>X</label>
                  <input type="number" className="ribbon-input" value={actions.translateX} onChange={(e) => actions.setTranslateX(Number(e.target.value))} step="1" />
                </div>
                <div className="ribbon-input-row">
                  <label>Y</label>
                  <input type="number" className="ribbon-input" value={actions.translateY} onChange={(e) => actions.setTranslateY(Number(e.target.value))} step="1" />
                </div>
                <div className="ribbon-input-row">
                  <label>Z</label>
                  <input type="number" className="ribbon-input" value={actions.translateZ} onChange={(e) => actions.setTranslateZ(Number(e.target.value))} step="1" />
                </div>
              </div>
              <RibbonButton icon={<Move size={20} />} label="Apply" onClick={actions.handleTranslate} disabled={!hasActivePointcloud} tooltip="Translate pointcloud" />
            </RibbonGroup>

            <RibbonGroup label="Scale">
              <div className="ribbon-transform-inputs">
                <div className="ribbon-input-row">
                  <label>X</label>
                  <input type="number" className="ribbon-input" value={actions.scaleX} onChange={(e) => actions.setScaleX(Number(e.target.value))} step="0.1" />
                </div>
                <div className="ribbon-input-row">
                  <label>Y</label>
                  <input type="number" className="ribbon-input" value={actions.scaleY} onChange={(e) => actions.setScaleY(Number(e.target.value))} step="0.1" />
                </div>
                <div className="ribbon-input-row">
                  <label>Z</label>
                  <input type="number" className="ribbon-input" value={actions.scaleZ} onChange={(e) => actions.setScaleZ(Number(e.target.value))} step="0.1" />
                </div>
              </div>
              <RibbonButton icon={<Maximize size={20} />} label="Apply" onClick={actions.handleScale} disabled={!hasActivePointcloud} tooltip="Scale pointcloud around centroid" />
            </RibbonGroup>

            <RibbonGroup label="Thin">
              <div className="ribbon-transform-inputs">
                <div className="ribbon-input-row">
                  <label>%</label>
                  <input type="range" min="1" max="100" value={actions.thinPercent} onChange={(e) => actions.setThinPercent(Number(e.target.value))} className="w-16" />
                  <span className="ribbon-input-value">{actions.thinPercent}%</span>
                </div>
              </div>
              <RibbonButton icon={<Filter size={20} />} label="Apply" onClick={actions.handleThin} disabled={!hasActivePointcloud} tooltip={`Keep ${actions.thinPercent}% of points`} />
            </RibbonGroup>

            <RibbonGroup label="Surface">
              <RibbonButton icon={<Shapes size={20} />} label={actions.reconstructing ? 'Working...' : 'Reconstruct'} onClick={actions.handleReconstruct} disabled={!hasActivePointcloud || actions.reconstructing} tooltip="Reconstruct triangle mesh from pointcloud" />
              <RibbonButton icon={<Download size={20} />} label="Export OBJ" onClick={actions.handleExportOBJ} disabled={!hasActivePointcloud} tooltip="Export mesh as Wavefront OBJ file" />
            </RibbonGroup>

            <RibbonGroup label="Extensions">
              <RibbonButton icon={<Building2 size={20} />} label="3D BAG" onClick={() => setShowBAG3DPanel(!showBAG3DPanel)} active={showBAG3DPanel} tooltip="Download 3D buildings from 3dbag.nl" />
            </RibbonGroup>
          </div>
        </div>
      </div>

      </div>
    <ReconstructionProgressDialog
      open={actions.reconOpen}
      phase={actions.reconPhase}
      percent={actions.reconPercent}
      onCancel={actions.handleReconCancel}
    />
    </>
  );
});
