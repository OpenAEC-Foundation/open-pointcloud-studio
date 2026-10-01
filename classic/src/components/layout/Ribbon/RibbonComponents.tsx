import { useState, useRef, useEffect, useCallback } from 'react';
import { Check, ChevronDown } from 'lucide-react';
import { type UITheme, UI_THEMES } from '../../../state/appStore';

// ============================================================================
// Tooltip
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

// ============================================================================
// useTooltip Hook
// ============================================================================

export function useTooltip(delay = 400) {
  const [show, setShow] = useState(false);
  const ref = useRef<HTMLButtonElement>(null);
  const timerRef = useRef<ReturnType<typeof setTimeout>>(undefined);

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

// ============================================================================
// RibbonButton
// ============================================================================

interface RibbonButtonProps {
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
  active?: boolean;
  disabled?: boolean;
  shortcut?: string;
  tooltip?: string;
}

export function RibbonButton({ icon, label, onClick, active, disabled, shortcut, tooltip }: RibbonButtonProps) {
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

// ============================================================================
// RibbonSmallButton
// ============================================================================

interface RibbonSmallButtonProps {
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
  active?: boolean;
  disabled?: boolean;
  shortcut?: string;
}

export function RibbonSmallButton({ icon, label, onClick, active, disabled, shortcut }: RibbonSmallButtonProps) {
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

// ============================================================================
// RibbonGroup & RibbonButtonStack
// ============================================================================

export function RibbonGroup({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="ribbon-group">
      <div className="ribbon-group-content">{children}</div>
      <div className="ribbon-group-label">{label}</div>
    </div>
  );
}

export function RibbonButtonStack({ children }: { children: React.ReactNode }) {
  return <div className="ribbon-btn-stack">{children}</div>;
}

// ============================================================================
// RibbonDropdownButton
// ============================================================================

interface RibbonDropdownButtonProps {
  icon: React.ReactNode;
  label: string;
  disabled?: boolean;
  tooltip?: string;
  items: { label: string; onClick: () => void }[];
}

export function RibbonDropdownButton({ icon, label, disabled, tooltip, items }: RibbonDropdownButtonProps) {
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
// ThemeSelector
// ============================================================================

interface ThemeSelectorProps {
  currentTheme: UITheme;
  onThemeChange: (theme: UITheme) => void;
}

export function ThemeSelector({ currentTheme, onThemeChange }: ThemeSelectorProps) {
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
