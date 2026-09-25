# UI Rebrand — Volledige adoptie @openaec/ui (Plan 1 van 5)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Vervang de huidige multi-theme styling (dark/light/blue/highContrast met eigen `--theme-*` variabelen) volledig door de OpenAEC-Foundation design system (`@openaec/ui`) als enige bron van tokens en componenten. Eén thema, oranje accent, donker-grijs basis. Geen hybride oplossing, geen aliases.

**Architecture:** `@openaec/ui` wordt als npm-dependency uit de GitHub-repo geïnstalleerd (`OpenAEC-Foundation/openaec-ui`). Zijn `tokens.css` (CSS custom properties met prefix `--oaec-*`) en `components.css` (klassen met prefix `.oaec-*`) worden in `src/main.tsx` geïmporteerd. Alle bestaande `--theme-*` referenties in onze CSS worden hernoemd naar de equivalente `--oaec-*` tokens. App-specifieke afgeleide tokens (ribbon-gradient, grid-kleuren) worden gedefinieerd in een nieuw bestand `src/styles/openaec-overrides.css` dat alleen `--oaec-*` tokens consumeert. Het theme-switch UI en alle bijbehorende state wordt verwijderd.

**Tech Stack:** React 19, Vite 7, Tauri 2, TailwindCSS 4, `@openaec/ui` (CSS only).

---

## Bestandsstructuur

**Aanmaken:**
- `src/styles/openaec-overrides.css` — app-specifieke composietsstoken bovenop `@openaec/ui`

**Aanpassen:**
- `package.json` — `@openaec/ui` als dependency vanaf GitHub
- `src/main.tsx` — imports van openaec CSS
- `index.html` — `data-theme="light"` op `<html>`
- `src/styles/globals.css` — huidige `--theme-*` blokken eruit, `--oaec-*` consumptie + utility-classes blijven
- `src/components/layout/Ribbon/Ribbon.css` — `--theme-*` → `--oaec-*` rename
- `src/components/SettingsDialog/SettingsDialog.css` — `--theme-*` → `--oaec-*` rename + theme-selector classes weg
- `src/App.tsx` — theme-effect weg, geen useEffect meer voor `data-theme`
- `src/state/appStore.ts` — `UITheme`, `UI_THEMES`, `uiTheme`, `setUITheme` weg
- `src/components/SettingsDialog/SettingsDialog.tsx` — theme-picker sectie weg
- `src/components/layout/Ribbon/Ribbon.tsx` — theme cycle button + ThemeSelector mount weg
- `src/components/layout/Ribbon/RibbonComponents.tsx` — `ThemeSelector` component-export weg
- `src-tauri/tauri.conf.json` — window background color naar `#36363E`

**Verwijderen** (na rename, in laatste task):
- Geen bestanden zelf, alle dood code wordt uit modules weggehaald

---

## Token-mapping referentie (oude → nieuwe naam)

Deze tabel is canoniek voor alle rename-taken in dit plan. Wijk hier niet van af.

| `--theme-*` (oud) | `--oaec-*` (nieuw) |
|---|---|
| `--theme-bg` | `--oaec-bg` |
| `--theme-surface` | `--oaec-bg-lighter` |
| `--theme-surface-elevated` | `--oaec-bg-lighter` |
| `--theme-border` | `--oaec-border` |
| `--theme-border-light` | `--oaec-border-subtle` |
| `--theme-accent` | `--oaec-accent` |
| `--theme-accent-hover` | `--oaec-accent-hover` |
| `--theme-text` | `--oaec-text` |
| `--theme-text-dim` | `--oaec-text-secondary` |
| `--theme-text-muted` | `--oaec-text-muted` |
| `--theme-input-bg` | `--oaec-bg-input` |
| `--theme-hover` | `--oaec-accent-soft` |
| `--theme-active` | `--oaec-accent` |
| `--theme-active-border` | `--oaec-accent` |
| `--theme-dropdown-bg` | `--oaec-bg-lighter` |
| `--theme-scrollbar-track` | `--oaec-scrollbar-track` |
| `--theme-scrollbar-thumb` | `--oaec-scrollbar-thumb` |
| `--theme-grid` | `--oaec-grid` (app-specifiek, def in openaec-overrides.css) |
| `--theme-grid-major` | `--oaec-grid-major` (app-specifiek) |
| `--theme-ribbon-bg` | `--oaec-ribbon-bg` (app-specifiek) |
| `--theme-ribbon-tab-bg` | `--oaec-ribbon-tab-bg` (app-specifiek) |
| `--theme-ribbon-content-bg` | `--oaec-bg` |
| `--theme-file-tab-bg` | `--oaec-accent` |
| `--theme-file-tab-hover` | `--oaec-accent-hover` |
| `--theme-bg-panel` (fallback) | `--oaec-bg-lighter` |
| `--theme-bg-deep` (fallback) | `--oaec-bg-input` |

---

## Task 1: Dependency installeren + smoke-import

**Files:**
- Modify: `package.json`
- Modify: `src/main.tsx`
- Modify: `index.html`

- [ ] **Step 1: Voeg `@openaec/ui` toe als GitHub-dependency**

Voer uit in de repo root:

```bash
npm install --save OpenAEC-Foundation/openaec-ui
```

Verifieer dat `package.json` nu deze entry bevat onder `dependencies`:

```json
"@openaec/ui": "github:OpenAEC-Foundation/openaec-ui",
```

(Het exacte semver-spec mag varieren; punt is dat het via github wordt resolved.)

- [ ] **Step 2: Verifieer dat de package klopt**

```bash
ls node_modules/@openaec/ui/css
```

Verwacht: `components.css  tokens.css`

Als dit faalt: stop het plan, fix npm install eerst.

- [ ] **Step 3: Importeer openaec CSS in main.tsx (boven globals.css)**

Open `src/main.tsx`. Voeg deze twee regels toe **direct voor** de bestaande `import './styles/globals.css';`:

```ts
import '@openaec/ui/css/tokens.css';
import '@openaec/ui/css/components.css';
```

Bestaande `import './styles/globals.css';` blijft staan.

- [ ] **Step 4: Zet data-theme op de root in index.html**

Open `index.html` (project root). Vind de `<html>` openingstag en wijzig naar:

```html
<html lang="en" data-theme="light">
```

Reden: OpenAEC's `tokens.css` activeert de oranje-accent + donker-grijs palette onder `[data-theme="light"]` (verwarrend benoemd in hun conventie, maar dat is hun keuze).

- [ ] **Step 5: Smoke-build**

```bash
npm run build
```

Expected: clean exit, geen TS errors of CSS parse errors.

Als de build faalt door CSS-conflicten: dat is OK voor nu, blijf bij dit step en los op. Het type-deel moet wél slagen.

- [ ] **Step 6: Commit**

```bash
git add package.json package-lock.json src/main.tsx index.html
git commit -m "ui: install @openaec/ui and wire tokens + components CSS"
```

---

## Task 2: Vervang globals.css door @openaec/ui consumptie + app-overrides

**Files:**
- Create: `src/styles/openaec-overrides.css`
- Modify: `src/styles/globals.css`
- Modify: `src/main.tsx`

- [ ] **Step 1: Maak `src/styles/openaec-overrides.css`**

Dit bestand definieert *alleen* app-specifieke composietstokens die niet in `@openaec/ui` zitten (ribbon-gradient, canvas-grid-kleuren). Alles bouwt op `--oaec-*` tokens, géén hardcoded hex codes.

Schrijf de inhoud:

```css
/*
 * App-specifieke tokens bovenop @openaec/ui.
 * Definieert composietsvariabelen die alleen open-pointcloud-studio gebruikt
 * (ribbon-gradient, canvas-grid). Geen kleur-hex hier; alles via oaec tokens.
 */

:root,
[data-theme="light"] {
  /* Canvas-grid (gebruikt door PointcloudViewer) */
  --oaec-grid: var(--oaec-bg-lighter);
  --oaec-grid-major: var(--oaec-border);

  /* Ribbon gradient (gebruikt door Ribbon.css) */
  --oaec-ribbon-bg: linear-gradient(
    to bottom,
    var(--oaec-bg-lighter) 0%,
    var(--oaec-bg) 100%
  );
  --oaec-ribbon-tab-bg: linear-gradient(
    to bottom,
    var(--oaec-bg-lighter) 0%,
    var(--oaec-bg) 100%
  );
}
```

- [ ] **Step 2: Importeer overrides in main.tsx**

Open `src/main.tsx`. Voeg toe **direct na** de openaec component import:

```ts
import '@openaec/ui/css/tokens.css';
import '@openaec/ui/css/components.css';
import './styles/openaec-overrides.css';   // <-- nieuw
import './styles/globals.css';
```

- [ ] **Step 3: Strip multi-theme blokken uit globals.css**

Open `src/styles/globals.css`. Vervang regels 20 t/m 169 (alles van de comment `/* ============================================================================` `Theme CSS Variables` tot en met het einde van het `[data-theme="highContrast"] .ribbon-btn.active` blok) door:

```css
/* ============================================================================
   Theme: single OpenAEC theme via @openaec/ui — geen alternatieven meer
   ============================================================================ */

:root,
[data-theme="light"] {
  font-family: var(--oaec-font-family);
  line-height: 1.5;
  font-weight: var(--oaec-weight-normal);
  color-scheme: dark;
  color: var(--oaec-text);
  background-color: var(--oaec-bg);

  font-synthesis: none;
  text-rendering: optimizeLegibility;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
}
```

Het Tailwind `@theme` block bovenaan (`@theme { --color-cad-bg: ... }`) blijft, maar wijzig alle binnenkant naar `var(--oaec-*)`:

Originele inhoud (regels 3–18):

```css
@theme {
  --color-cad-bg: var(--theme-bg);
  --color-cad-surface: var(--theme-surface);
  ...
}
```

Vervang door:

```css
@theme {
  --color-cad-bg: var(--oaec-bg);
  --color-cad-surface: var(--oaec-bg-lighter);
  --color-cad-surface-elevated: var(--oaec-bg-lighter);
  --color-cad-border: var(--oaec-border);
  --color-cad-border-light: var(--oaec-border-subtle);
  --color-cad-accent: var(--oaec-accent);
  --color-cad-text: var(--oaec-text);
  --color-cad-text-dim: var(--oaec-text-secondary);
  --color-cad-text-muted: var(--oaec-text-muted);
  --color-cad-grid: var(--oaec-grid);
  --color-cad-grid-major: var(--oaec-grid-major);
  --color-cad-hover: var(--oaec-accent-soft);
  --color-cad-input: var(--oaec-bg-input);
  --color-cad-dropdown: var(--oaec-bg-lighter);
}
```

- [ ] **Step 4: Rename overige `--theme-*` referenties in globals.css**

In het resterende deel van `globals.css` (vanaf de scrollbar regels, ~regel 185 in het originele bestand), vervang elke voorkomen van `--theme-*` volgens de mapping in de canonieke tabel bovenaan.

Gebruik je editor's find-replace per token. Of doe in één klap met sed:

```bash
sed -i \
  -e 's/var(--theme-bg-panel[^)]*)/var(--oaec-bg-lighter)/g' \
  -e 's/var(--theme-bg-deep[^)]*)/var(--oaec-bg-input)/g' \
  -e 's/--theme-bg\b/--oaec-bg/g' \
  -e 's/--theme-surface-elevated\b/--oaec-bg-lighter/g' \
  -e 's/--theme-surface\b/--oaec-bg-lighter/g' \
  -e 's/--theme-border-light\b/--oaec-border-subtle/g' \
  -e 's/--theme-border\b/--oaec-border/g' \
  -e 's/--theme-accent-hover\b/--oaec-accent-hover/g' \
  -e 's/--theme-accent\b/--oaec-accent/g' \
  -e 's/--theme-text-dim\b/--oaec-text-secondary/g' \
  -e 's/--theme-text-muted\b/--oaec-text-muted/g' \
  -e 's/--theme-text\b/--oaec-text/g' \
  -e 's/--theme-input-bg\b/--oaec-bg-input/g' \
  -e 's/--theme-hover\b/--oaec-accent-soft/g' \
  -e 's/--theme-active-border\b/--oaec-accent/g' \
  -e 's/--theme-active\b/--oaec-accent/g' \
  -e 's/--theme-dropdown-bg\b/--oaec-bg-lighter/g' \
  -e 's/--theme-scrollbar-track\b/--oaec-scrollbar-track/g' \
  -e 's/--theme-scrollbar-thumb\b/--oaec-scrollbar-thumb/g' \
  -e 's/--theme-grid-major\b/--oaec-grid-major/g' \
  -e 's/--theme-grid\b/--oaec-grid/g' \
  -e 's/--theme-ribbon-content-bg\b/--oaec-bg/g' \
  -e 's/--theme-ribbon-tab-bg\b/--oaec-ribbon-tab-bg/g' \
  -e 's/--theme-ribbon-bg\b/--oaec-ribbon-bg/g' \
  -e 's/--theme-file-tab-hover\b/--oaec-accent-hover/g' \
  -e 's/--theme-file-tab-bg\b/--oaec-accent/g' \
  src/styles/globals.css
```

Op Windows zonder sed: doe het met find-replace in je editor in dezelfde volgorde (langere namen eerst om partial matches te voorkomen).

- [ ] **Step 5: Vervang hardcoded fallback hex-codes in globals.css**

Zoek in `globals.css` naar hex-codes die niet via tokens lopen. Vervang:

| Vind | Vervang door |
|---|---|
| `#1d4ed8` (in `.bag3d-download-btn` background) | `var(--oaec-accent)` |
| `#2563eb` (in `.bag3d-download-btn` border + hover) | `var(--oaec-accent-hover)` |
| `#ef4444` (in `.bag3d-error`) | `#ef4444` — **behouden** (error red is semantisch, niet brand-kleur) |
| `#fff` in `.bag3d-draw-btn.active` color | `var(--oaec-accent-text)` |
| `#fff` in `.bag3d-download-btn` color | `var(--oaec-accent-text)` |

- [ ] **Step 6: Verifieer geen --theme-* meer in globals.css**

```bash
grep -n "\-\-theme\-" src/styles/globals.css
```

Expected: geen output. Zo niet: handmatig nalopen welke je miste, fix.

- [ ] **Step 7: Build smoke-test**

```bash
npm run build
```

Expected: clean exit.

- [ ] **Step 8: Commit**

```bash
git add src/styles/globals.css src/styles/openaec-overrides.css src/main.tsx
git commit -m "ui: replace --theme-* with @openaec/ui --oaec-* tokens in globals.css"
```

---

## Task 3: Rename `--theme-*` naar `--oaec-*` in Ribbon.css

**Files:**
- Modify: `src/components/layout/Ribbon/Ribbon.css`

- [ ] **Step 1: Sed-vervanging (zelfde mapping als Task 2)**

```bash
sed -i \
  -e 's/var(--theme-bg-panel[^)]*)/var(--oaec-bg-lighter)/g' \
  -e 's/var(--theme-bg-deep[^)]*)/var(--oaec-bg-input)/g' \
  -e 's/--theme-bg\b/--oaec-bg/g' \
  -e 's/--theme-surface-elevated\b/--oaec-bg-lighter/g' \
  -e 's/--theme-surface\b/--oaec-bg-lighter/g' \
  -e 's/--theme-border-light\b/--oaec-border-subtle/g' \
  -e 's/--theme-border\b/--oaec-border/g' \
  -e 's/--theme-accent-hover\b/--oaec-accent-hover/g' \
  -e 's/--theme-accent\b/--oaec-accent/g' \
  -e 's/--theme-text-dim\b/--oaec-text-secondary/g' \
  -e 's/--theme-text-muted\b/--oaec-text-muted/g' \
  -e 's/--theme-text\b/--oaec-text/g' \
  -e 's/--theme-input-bg\b/--oaec-bg-input/g' \
  -e 's/--theme-hover\b/--oaec-accent-soft/g' \
  -e 's/--theme-active-border\b/--oaec-accent/g' \
  -e 's/--theme-active\b/--oaec-accent/g' \
  -e 's/--theme-dropdown-bg\b/--oaec-bg-lighter/g' \
  -e 's/--theme-scrollbar-track\b/--oaec-scrollbar-track/g' \
  -e 's/--theme-scrollbar-thumb\b/--oaec-scrollbar-thumb/g' \
  -e 's/--theme-grid-major\b/--oaec-grid-major/g' \
  -e 's/--theme-grid\b/--oaec-grid/g' \
  -e 's/--theme-ribbon-content-bg\b/--oaec-bg/g' \
  -e 's/--theme-ribbon-tab-bg\b/--oaec-ribbon-tab-bg/g' \
  -e 's/--theme-ribbon-bg\b/--oaec-ribbon-bg/g' \
  -e 's/--theme-file-tab-hover\b/--oaec-accent-hover/g' \
  -e 's/--theme-file-tab-bg\b/--oaec-accent/g' \
  src/components/layout/Ribbon/Ribbon.css
```

- [ ] **Step 2: Verifieer geen `--theme-*` meer**

```bash
grep -n "\-\-theme\-" src/components/layout/Ribbon/Ribbon.css
```

Expected: geen output.

- [ ] **Step 3: Build smoke-test**

```bash
npm run build
```

Expected: clean exit.

- [ ] **Step 4: Commit**

```bash
git add src/components/layout/Ribbon/Ribbon.css
git commit -m "ui: rename --theme-* to --oaec-* in Ribbon.css"
```

---

## Task 4: Rename `--theme-*` naar `--oaec-*` in SettingsDialog.css

**Files:**
- Modify: `src/components/SettingsDialog/SettingsDialog.css`

- [ ] **Step 1: Sed-vervanging (zelfde mapping)**

Voer hetzelfde sed-commando uit als in Task 3 stap 1, maar dan op `src/components/SettingsDialog/SettingsDialog.css`.

- [ ] **Step 2: Verifieer**

```bash
grep -n "\-\-theme\-" src/components/SettingsDialog/SettingsDialog.css
```

Expected: geen output.

- [ ] **Step 3: Build smoke-test**

```bash
npm run build
```

Expected: clean exit.

- [ ] **Step 4: Commit**

```bash
git add src/components/SettingsDialog/SettingsDialog.css
git commit -m "ui: rename --theme-* to --oaec-* in SettingsDialog.css"
```

---

## Task 5: Verwijder theme-state uit appStore.ts

**Files:**
- Modify: `src/state/appStore.ts`

- [ ] **Step 1: Lees het bestand om het exacte type/structuur te zien**

```bash
grep -n "UITheme\|UI_THEMES\|uiTheme\|setUITheme" src/state/appStore.ts
```

Noteer alle regels — die gaan eruit.

- [ ] **Step 2: Verwijder UITheme type-export (regel 21)**

Open `src/state/appStore.ts`. Verwijder:

```ts
export type UITheme = 'dark' | 'light' | 'blue' | 'highContrast';
```

- [ ] **Step 3: Verwijder UI_THEMES constant-export (rond regel 23–29)**

Verwijder het hele `UI_THEMES` array-blok.

- [ ] **Step 4: Verwijder `uiTheme` veld uit de state-interface**

In het `interface AppState` (of vergelijkbare naam, rond regel 31), verwijder de regel:

```ts
uiTheme: UITheme;
```

- [ ] **Step 5: Verwijder `setUITheme` uit de actions-interface**

In dezelfde interface, verwijder:

```ts
setUITheme: (theme: UITheme) => void;
```

- [ ] **Step 6: Verwijder de initial-state regel**

Rond regel 43, verwijder:

```ts
uiTheme: 'dark',
```

- [ ] **Step 7: Verwijder de setter-implementatie**

Rond regel 60–62, verwijder:

```ts
setUITheme: (theme: UITheme) => {
  set((s) => { s.uiTheme = theme; });
},
```

- [ ] **Step 8: Verifieer geen UITheme/uiTheme meer in appStore.ts**

```bash
grep -n "UITheme\|UI_THEMES\|uiTheme\|setUITheme" src/state/appStore.ts
```

Expected: geen output.

- [ ] **Step 9: Type-check (gaat falen elders, dat is verwacht)**

```bash
npx tsc --noEmit
```

Expected: errors in `App.tsx`, `Ribbon.tsx`, `RibbonComponents.tsx`, `SettingsDialog.tsx` (over ontbrekende `UITheme` export). Die fixen we in de volgende tasks. Nog niet committen.

---

## Task 6: Verwijder theme-effect uit App.tsx

**Files:**
- Modify: `src/App.tsx`

- [ ] **Step 1: Lees regel 12–18**

```bash
sed -n '10,20p' src/App.tsx
```

Verwacht ongeveer:

```tsx
function App() {
  // Apply theme on mount
  const uiTheme = useAppStore((s) => s.uiTheme);
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', uiTheme);
  }, [uiTheme]);
```

- [ ] **Step 2: Verwijder de theme-effect (3 regels uiTheme + useEffect)**

Open `src/App.tsx`. Verwijder regels:

```tsx
  // Apply theme on mount
  const uiTheme = useAppStore((s) => s.uiTheme);
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', uiTheme);
  }, [uiTheme]);
```

Reden: `data-theme="light"` staat nu vast in `index.html` (Task 1 stap 4); geen runtime-switching meer.

- [ ] **Step 3: Verwijder ongebruikte React-imports indien `useEffect` nergens anders gebruikt wordt**

```bash
grep -c "useEffect" src/App.tsx
```

Als 0: in de top-import regel verwijder `useEffect` uit de React-import lijst. Als ≥1: laat staan.

- [ ] **Step 4: Type-check op alleen App.tsx**

```bash
npx tsc --noEmit
```

Expected: minder errors dan eerder. App.tsx zelf moet clean zijn. Errors in Ribbon/SettingsDialog/RibbonComponents blijven — dat is volgende task.

---

## Task 7: Verwijder theme-selector uit Ribbon.tsx en RibbonComponents.tsx

**Files:**
- Modify: `src/components/layout/Ribbon/Ribbon.tsx`
- Modify: `src/components/layout/Ribbon/RibbonComponents.tsx`

- [ ] **Step 1: Verwijder ThemeSelector-gebruik uit Ribbon.tsx**

Open `src/components/layout/Ribbon/Ribbon.tsx`. Vind regels rond 24–25, 143–149.

Verwijder:

```tsx
import { type UITheme } from '../../../state/appStore';
```

(of pas de import-regel aan zodat alleen overige imports overblijven; als deze regel een gecombineerde import was, hou de rest)

Verwijder:

```tsx
const uiTheme = useAppStore((s) => s.uiTheme);
const setUITheme = useAppStore((s) => s.setUITheme);
```

Vind het blok met de theme-cycle-button (rond regel 143):

```tsx
const themes: UITheme[] = ['dark', 'light', 'blue', 'highContrast'];
const idx = themes.indexOf(uiTheme);
setUITheme(themes[(idx + 1) % themes.length]);
```

Verwijder de hele knop waarin dit zit (de surrounding `<button>` of `<RibbonButton>`).

Verwijder de `<ThemeSelector currentTheme={uiTheme} onThemeChange={setUITheme} />` regel.

- [ ] **Step 2: Verwijder ThemeSelector uit RibbonComponents.tsx**

Open `src/components/layout/Ribbon/RibbonComponents.tsx`. Vind:

```tsx
import { type UITheme, UI_THEMES } from '../../../state/appStore';
```

Verwijder die import-regel.

Vind het ThemeSelector-component (regels rond 268+):

```tsx
currentTheme: UITheme;
onThemeChange: (theme: UITheme) => void;
```

Verwijder het hele `ThemeSelector` component (export + props-interface) — meestal een blok van 30–60 regels.

- [ ] **Step 3: Type-check**

```bash
npx tsc --noEmit
```

Expected: alleen nog errors in SettingsDialog.tsx — die fixen we in volgende task.

---

## Task 8: Verwijder theme-picker uit SettingsDialog.tsx

**Files:**
- Modify: `src/components/SettingsDialog/SettingsDialog.tsx`

- [ ] **Step 1: Verwijder UITheme-import**

Open `src/components/SettingsDialog/SettingsDialog.tsx`. Vind regel 2:

```tsx
import { useAppStore, type UITheme } from '../../state/appStore';
```

Wijzig naar:

```tsx
import { useAppStore } from '../../state/appStore';
```

(Of indien `useAppStore` ook niet meer nodig is na deze refactor, verwijder de hele regel.)

- [ ] **Step 2: Verwijder de THEMES constant (regels 5–13)**

Verwijder het hele `const THEMES: { value: UITheme; label: string; swatches: string[] }[] = [...]` blok.

- [ ] **Step 3: Verwijder theme-state en handler**

Verwijder regels:

```tsx
const uiTheme = useAppStore((s) => s.uiTheme);
const setUITheme = useAppStore((s) => s.setUITheme);
```

En:

```tsx
const handleThemeChange = (theme: UITheme) => {
  setUITheme(theme);
};
```

- [ ] **Step 4: Verwijder de theme-picker JSX-sectie**

Vind in de JSX-output (rond regel 91) de map over `THEMES.map(theme => ...)` met className `settings-theme-row`. Verwijder die hele sectie inclusief de section-header zoals "Thema" of vergelijkbaar.

Als de volledige `<section>` of `<fieldset>` nu leeg is, verwijder die hele wrapper ook.

- [ ] **Step 5: Verwijder de theme-row CSS uit SettingsDialog.css**

In `src/components/SettingsDialog/SettingsDialog.css`, vind alle regels met `.settings-theme-row` (active state, swatches, etc.) en verwijder ze.

```bash
grep -n "settings-theme-row\|theme-swatch" src/components/SettingsDialog/SettingsDialog.css
```

Verwijder die regelblokken in de editor.

- [ ] **Step 6: Full type-check**

```bash
npx tsc --noEmit
```

Expected: clean. Geen errors meer.

- [ ] **Step 7: Full build**

```bash
npm run build
```

Expected: clean exit.

- [ ] **Step 8: Commit (alles van Task 5 t/m 8)**

```bash
git add \
  src/state/appStore.ts \
  src/App.tsx \
  src/components/layout/Ribbon/Ribbon.tsx \
  src/components/layout/Ribbon/RibbonComponents.tsx \
  src/components/SettingsDialog/SettingsDialog.tsx \
  src/components/SettingsDialog/SettingsDialog.css
git commit -m "ui: remove multi-theme switcher — single OpenAEC theme via index.html"
```

---

## Task 9: Tauri window background + finale CSS-controle

**Files:**
- Modify: `src-tauri/tauri.conf.json`

- [ ] **Step 1: Update window background-color**

Open `src-tauri/tauri.conf.json`. Vind het `app.windows` array. Voeg of update in elke window-config:

```json
{
  "title": "Open Pointcloud Studio",
  "width": 1400,
  "height": 900,
  "backgroundColor": "#36363E"
}
```

`#36363E` is exact de waarde van `--oaec-bg` in `[data-theme="light"]`. Dit voorkomt witte flash bij app-startup voordat de WebView is geladen.

- [ ] **Step 2: Verifieer geen `--theme-*` referenties meer in heel src/**

```bash
grep -rn "\-\-theme\-" src/
```

Expected: geen output.

- [ ] **Step 3: Verifieer geen UITheme-referenties meer in heel src/**

```bash
grep -rn "UITheme\|UI_THEMES\|uiTheme\|setUITheme" src/
```

Expected: geen output.

- [ ] **Step 4: Verifieer geen `data-theme=` switching logica meer**

```bash
grep -rn "data-theme" src/
```

Expected: alleen `index.html` (static), evt. een inline-comment elders. Geen `setAttribute('data-theme', ...)` calls.

- [ ] **Step 5: Full build**

```bash
npm run build
```

Expected: clean.

- [ ] **Step 6: Visuele smoke-test in dev**

```bash
npm run tauri dev
```

Wacht tot de app opent. Controleer visueel:

1. Achtergrond is donker-grijs (`#36363E`), niet blauw of zwart.
2. Accent-kleur is oranje (`#D97706`) — op buttons, active states.
3. Geen residuale blauwe accent (`#e94560` / `#a82d6e` / `#3b82f6`) zichtbaar.
4. Ribbon-tabs hebben de juiste donker-grijs gradient.
5. Panels en dialogs hebben de juiste oranje-getinte borders.
6. Settings dialog opent en bevat **geen** theme-picker sectie meer.

Als één van deze faalt: noteer welke component, fix het token erin, commit als losse fix.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/tauri.conf.json
git commit -m "ui: set Tauri window background to OpenAEC base color"
```

---

## Task 10: README + screenshot bijwerken (optioneel)

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update README Features-sectie**

Open `README.md`. In de Features-lijst, verwijder de regel:

```
- Dark, Light, Blue, and High Contrast themes
```

Vervang door:

```
- OpenAEC Foundation design system (one cohesive theme)
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: update README features for single OpenAEC theme"
```

---

## Acceptatie-criteria voor Plan 1

Het plan is "klaar" wanneer:

1. ✅ `grep -rn "\-\-theme\-" src/` levert nul resultaten.
2. ✅ `grep -rn "UITheme\|UI_THEMES\|uiTheme\|setUITheme" src/` levert nul resultaten.
3. ✅ `npm run build` slaagt zonder errors of warnings over CSS/types.
4. ✅ `npm run tauri dev` toont een werkende app met OpenAEC look-and-feel: donker-grijs basis, oranje accent.
5. ✅ Settings-dialog opent en bevat geen theme-picker meer.
6. ✅ Ribbon toont geen theme-cycle-knop meer.
7. ✅ Geen hardcoded brand-kleuren meer in CSS — alleen semantische kleuren (zoals error-rood) mogen blijven.
8. ✅ Tauri window-flash bij startup is grijs, niet wit.
9. ✅ Alle commits zijn gepushed naar `main` of een feature-branch.

---

## Wat dit plan NIET doet (komt in latere plannen)

- COPC-streaming pipeline (Plan 2)
- CCCoreLib FFI-bridge (Plan 3)
- Edit-mask + selectie (Plan 4)
- Scan-stations (Plan 5)
- Verwijderen van mesh-parsers en oude pointcloud-pipeline (Plan 2)
- Ribbon-acties herstructureren naar nieuwe IPC-commands (Plan 2)
