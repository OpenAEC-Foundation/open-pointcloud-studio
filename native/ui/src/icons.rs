//! Canonical semantic icons. Source SVG geometry is embedded, never fetched at runtime.
use iced::{widget::svg, Color};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    New,
    Open,
    Save,
    SaveAs,
    Print,
    Import,
    Export,
    Undo,
    Redo,
    Settings,
    Controls,
    Close,
    Minimize,
    Maximize,
    Restore,
    PanelLeft,
    PanelRight,
    Search,
    Measure,
    Fit,
    FitSelection,
    ZoomIn,
    ZoomOut,
    Pan,
    Cube,
    Extensions,
    Help,
    Back,
    Check,
    Grid,
    Microphone,
    Duplicate,
    Rename,
    DisclosureExpanded,
    DisclosureCollapsed,
}
impl Icon {
    pub const ALL: [Self; 35] = [
        Self::New,
        Self::Open,
        Self::Save,
        Self::SaveAs,
        Self::Print,
        Self::Import,
        Self::Export,
        Self::Undo,
        Self::Redo,
        Self::Settings,
        Self::Controls,
        Self::Close,
        Self::Minimize,
        Self::Maximize,
        Self::Restore,
        Self::PanelLeft,
        Self::PanelRight,
        Self::Search,
        Self::Measure,
        Self::Fit,
        Self::FitSelection,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::Pan,
        Self::Cube,
        Self::Extensions,
        Self::Help,
        Self::Back,
        Self::Check,
        Self::Grid,
        Self::Microphone,
        Self::Duplicate,
        Self::Rename,
        Self::DisclosureExpanded,
        Self::DisclosureCollapsed,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Open => "open",
            Self::Save => "save",
            Self::SaveAs => "save-as",
            Self::Print => "print",
            Self::Import => "import",
            Self::Export => "export",
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::Settings => "settings",
            Self::Controls => "controls",
            Self::Close => "close",
            Self::Minimize => "minimize",
            Self::Maximize => "maximize",
            Self::Restore => "restore",
            Self::PanelLeft => "panel-left",
            Self::PanelRight => "panel-right",
            Self::Search => "search",
            Self::Measure => "measure",
            Self::Fit => "fit",
            Self::FitSelection => "fit-selection",
            Self::ZoomIn => "zoom-in",
            Self::ZoomOut => "zoom-out",
            Self::Pan => "pan",
            Self::Cube => "cube",
            Self::Extensions => "extensions",
            Self::Help => "help",
            Self::Back => "back",
            Self::Check => "check",
            Self::Grid => "grid",
            Self::Microphone => "microphone",
            Self::Duplicate => "duplicate",
            Self::Rename => "rename",
            Self::DisclosureExpanded => "disclosure-expanded",
            Self::DisclosureCollapsed => "disclosure-collapsed",
        }
    }
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Self::New => include_bytes!("../assets/icons/new.svg"),
            Self::Open => include_bytes!("../assets/icons/open.svg"),
            Self::Save => include_bytes!("../assets/icons/save.svg"),
            Self::SaveAs => include_bytes!("../assets/icons/saveas.svg"),
            Self::Print => include_bytes!("../assets/icons/print.svg"),
            Self::Import => include_bytes!("../assets/icons/import.svg"),
            Self::Export => include_bytes!("../assets/icons/export.svg"),
            Self::Undo => include_bytes!("../assets/icons/undo.svg"),
            Self::Redo => include_bytes!("../assets/icons/redo.svg"),
            Self::Settings => include_bytes!("../assets/icons/settings.svg"),
            Self::Controls => include_bytes!("../assets/icons/controls.svg"),
            Self::Close => include_bytes!("../assets/icons/close.svg"),
            Self::Minimize => include_bytes!("../assets/icons/minimize.svg"),
            Self::Maximize => include_bytes!("../assets/icons/maximize.svg"),
            Self::Restore => include_bytes!("../assets/icons/restore.svg"),
            Self::PanelLeft => include_bytes!("../assets/icons/panelleft.svg"),
            Self::PanelRight => include_bytes!("../assets/icons/panelright.svg"),
            Self::Search => include_bytes!("../assets/icons/search.svg"),
            Self::Measure => include_bytes!("../assets/icons/measure.svg"),
            Self::Fit => include_bytes!("../assets/icons/fit.svg"),
            Self::FitSelection => include_bytes!("../assets/icons/fitselection.svg"),
            Self::ZoomIn => include_bytes!("../assets/icons/zoomin.svg"),
            Self::ZoomOut => include_bytes!("../assets/icons/zoomout.svg"),
            Self::Pan => include_bytes!("../assets/icons/pan.svg"),
            Self::Cube => include_bytes!("../assets/icons/cube.svg"),
            Self::Extensions => include_bytes!("../assets/icons/extensions.svg"),
            Self::Help => include_bytes!("../assets/icons/help.svg"),
            Self::Back => include_bytes!("../assets/icons/back.svg"),
            Self::Check => include_bytes!("../assets/icons/check.svg"),
            Self::Grid => include_bytes!("../assets/icons/grid.svg"),
            Self::Microphone => include_bytes!("../assets/icons/microphone.svg"),
            Self::Duplicate => include_bytes!("../assets/icons/duplicate.svg"),
            Self::Rename => include_bytes!("../assets/icons/rename.svg"),
            Self::DisclosureExpanded => include_bytes!("../assets/icons/disclosure-expanded.svg"),
            Self::DisclosureCollapsed => include_bytes!("../assets/icons/disclosure-collapsed.svg"),
        }
    }
    pub fn view<'a>(self, size: u16, color: Color) -> svg::Svg<'a> {
        svg(svg::Handle::from_memory(self.bytes()))
            .width(size)
            .height(size)
            .style(move |_, _| svg::Style { color: Some(color) })
    }
}

/// Reviewed tool-specific commands. Their names include the domain so a
/// visually similar shape cannot silently acquire another command meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainIcon {
    PointcloudBagPanel,
    PointcloudEyeDome,
    CadShadedView,
    CadSpherePrimitive,
    GeotechSondering,
    GeotechBoring,
    GeotechSonderingGrid,
    DrawingDimension,
    DrawingNote,
    TransformMove,
    DrawingLine,
    TransformRotate,
    DrawingTrim,
    DrawingExtend,
    TransformMirror,
    DrawingOffset,
    GeotechCoordinateTag,
    DrawingPolygon,
    DrawingSvgFrame,
    DrawingText,
    DrawingLeader,
    PdfFitWidth,
    PdfActualSize,
    PdfEditText,
    PdfClearPage,
    PdfDeletePage,
    PdfSinglePage,
    PdfContinuousPages,
    PdfFileAssociation,
    PdfBookView,
    PdfShortcuts,
    PdfNavigation,
    PdfManageWatermarks,
    PdfIfcReportExport,
    PdfProofread,
    PdfSplitDocument,
}

impl DomainIcon {
    pub const ALL: [Self; 36] = [
        Self::PointcloudBagPanel,
        Self::PointcloudEyeDome,
        Self::CadShadedView,
        Self::CadSpherePrimitive,
        Self::GeotechSondering,
        Self::GeotechBoring,
        Self::GeotechSonderingGrid,
        Self::DrawingDimension,
        Self::DrawingNote,
        Self::TransformMove,
        Self::DrawingLine,
        Self::TransformRotate,
        Self::DrawingTrim,
        Self::DrawingExtend,
        Self::TransformMirror,
        Self::DrawingOffset,
        Self::GeotechCoordinateTag,
        Self::DrawingPolygon,
        Self::DrawingSvgFrame,
        Self::DrawingText,
        Self::DrawingLeader,
        Self::PdfFitWidth,
        Self::PdfActualSize,
        Self::PdfEditText,
        Self::PdfClearPage,
        Self::PdfDeletePage,
        Self::PdfSinglePage,
        Self::PdfContinuousPages,
        Self::PdfFileAssociation,
        Self::PdfBookView,
        Self::PdfShortcuts,
        Self::PdfNavigation,
        Self::PdfManageWatermarks,
        Self::PdfIfcReportExport,
        Self::PdfProofread,
        Self::PdfSplitDocument,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::PointcloudBagPanel => "pointcloud.bag-panel",
            Self::PointcloudEyeDome => "pointcloud.eye-dome",
            Self::CadShadedView => "cad.shaded-view",
            Self::CadSpherePrimitive => "cad.sphere-primitive",
            Self::GeotechSondering => "geotech.sondering",
            Self::GeotechBoring => "geotech.boring",
            Self::GeotechSonderingGrid => "geotech.sondering-grid",
            Self::DrawingDimension => "drawing.dimension",
            Self::DrawingNote => "drawing.note",
            Self::TransformMove => "transform.move",
            Self::DrawingLine => "drawing.line",
            Self::TransformRotate => "transform.rotate",
            Self::DrawingTrim => "drawing.trim",
            Self::DrawingExtend => "drawing.extend",
            Self::TransformMirror => "transform.mirror",
            Self::DrawingOffset => "drawing.offset",
            Self::GeotechCoordinateTag => "geotech.coordinate-tag",
            Self::DrawingPolygon => "drawing.polygon",
            Self::DrawingSvgFrame => "drawing.svg-frame",
            Self::DrawingText => "drawing.text",
            Self::DrawingLeader => "drawing.leader",
            Self::PdfFitWidth => "pdf.fit-width",
            Self::PdfActualSize => "pdf.actual-size",
            Self::PdfEditText => "pdf.edit-text",
            Self::PdfClearPage => "pdf.clear-page",
            Self::PdfDeletePage => "pdf.delete-page",
            Self::PdfSinglePage => "pdf.single-page",
            Self::PdfContinuousPages => "pdf.continuous-pages",
            Self::PdfFileAssociation => "pdf.file-association",
            Self::PdfBookView => "pdf.book-view",
            Self::PdfShortcuts => "pdf.shortcuts",
            Self::PdfNavigation => "pdf.navigation",
            Self::PdfManageWatermarks => "pdf.manage-watermarks",
            Self::PdfIfcReportExport => "pdf.ifc-report-export",
            Self::PdfProofread => "pdf.proofread",
            Self::PdfSplitDocument => "pdf.split-document",
        }
    }

    pub fn bytes(self, size: u16) -> &'static [u8] {
        match (self, size <= 16) {
            (Self::PointcloudBagPanel, true) => {
                include_bytes!("../assets/domain-icons-small/pointcloud-bag-panel.svg")
            }
            (Self::PointcloudBagPanel, false) => {
                include_bytes!("../assets/domain-icons/pointcloud-bag-panel.svg")
            }
            (Self::PointcloudEyeDome, true) => {
                include_bytes!("../assets/domain-icons-small/pointcloud-edl.svg")
            }
            (Self::PointcloudEyeDome, false) => {
                include_bytes!("../assets/domain-icons/pointcloud-edl.svg")
            }
            (Self::CadShadedView, true) => {
                include_bytes!("../assets/domain-icons-small/cad-shaded.svg")
            }
            (Self::CadShadedView, false) => {
                include_bytes!("../assets/domain-icons/cad-shaded.svg")
            }
            (Self::CadSpherePrimitive, true) => {
                include_bytes!("../assets/domain-icons-small/cad-sphere.svg")
            }
            (Self::CadSpherePrimitive, false) => {
                include_bytes!("../assets/domain-icons/cad-sphere.svg")
            }
            (Self::GeotechSondering, true) => {
                include_bytes!("../assets/domain-icons-small/geotech-sondering.svg")
            }
            (Self::GeotechSondering, false) => {
                include_bytes!("../assets/domain-icons/geotech-sondering.svg")
            }
            (Self::GeotechBoring, true) => {
                include_bytes!("../assets/domain-icons-small/geotech-boring.svg")
            }
            (Self::GeotechBoring, false) => {
                include_bytes!("../assets/domain-icons/geotech-boring.svg")
            }
            (Self::GeotechSonderingGrid, true) => {
                include_bytes!("../assets/domain-icons-small/geotech-sondering-grid.svg")
            }
            (Self::GeotechSonderingGrid, false) => {
                include_bytes!("../assets/domain-icons/geotech-sondering-grid.svg")
            }
            (Self::DrawingDimension, true) => {
                include_bytes!("../assets/domain-icons-small/drawing-dimension.svg")
            }
            (Self::DrawingDimension, false) => {
                include_bytes!("../assets/domain-icons/drawing-dimension.svg")
            }
            (Self::DrawingNote, true) => {
                include_bytes!("../assets/domain-icons-small/drawing-note.svg")
            }
            (Self::DrawingNote, false) => {
                include_bytes!("../assets/domain-icons/drawing-note.svg")
            }
            (Self::TransformMove, true) => {
                include_bytes!("../assets/domain-icons-small/transform-move.svg")
            }
            (Self::TransformMove, false) => {
                include_bytes!("../assets/domain-icons/transform-move.svg")
            }
            (Self::DrawingLine, true) => {
                include_bytes!("../assets/domain-icons-small/drawing-line.svg")
            }
            (Self::DrawingLine, false) => {
                include_bytes!("../assets/domain-icons/drawing-line.svg")
            }
            (Self::TransformRotate, true) => {
                include_bytes!("../assets/domain-icons-small/transform-rotate.svg")
            }
            (Self::TransformRotate, false) => {
                include_bytes!("../assets/domain-icons/transform-rotate.svg")
            }
            (Self::DrawingTrim, true) => {
                include_bytes!("../assets/domain-icons-small/drawing-trim.svg")
            }
            (Self::DrawingTrim, false) => {
                include_bytes!("../assets/domain-icons/drawing-trim.svg")
            }
            (Self::DrawingExtend, true) => {
                include_bytes!("../assets/domain-icons-small/drawing-extend.svg")
            }
            (Self::DrawingExtend, false) => {
                include_bytes!("../assets/domain-icons/drawing-extend.svg")
            }
            (Self::TransformMirror, true) => {
                include_bytes!("../assets/domain-icons-small/transform-mirror.svg")
            }
            (Self::TransformMirror, false) => {
                include_bytes!("../assets/domain-icons/transform-mirror.svg")
            }
            (Self::DrawingOffset, true) => {
                include_bytes!("../assets/domain-icons-small/drawing-offset.svg")
            }
            (Self::DrawingOffset, false) => {
                include_bytes!("../assets/domain-icons/drawing-offset.svg")
            }
            (Self::GeotechCoordinateTag, true) => {
                include_bytes!("../assets/domain-icons-small/geotech-coordinate-tag.svg")
            }
            (Self::GeotechCoordinateTag, false) => {
                include_bytes!("../assets/domain-icons/geotech-coordinate-tag.svg")
            }
            (Self::DrawingPolygon, true) => {
                include_bytes!("../assets/domain-icons-small/drawing-polygon.svg")
            }
            (Self::DrawingPolygon, false) => {
                include_bytes!("../assets/domain-icons/drawing-polygon.svg")
            }
            (Self::DrawingSvgFrame, true) => {
                include_bytes!("../assets/domain-icons-small/drawing-svg-frame.svg")
            }
            (Self::DrawingSvgFrame, false) => {
                include_bytes!("../assets/domain-icons/drawing-svg-frame.svg")
            }
            (Self::DrawingText, true) => {
                include_bytes!("../assets/domain-icons-small/drawing-text.svg")
            }
            (Self::DrawingText, false) => {
                include_bytes!("../assets/domain-icons/drawing-text.svg")
            }
            (Self::DrawingLeader, true) => {
                include_bytes!("../assets/domain-icons-small/drawing-leader.svg")
            }
            (Self::DrawingLeader, false) => {
                include_bytes!("../assets/domain-icons/drawing-leader.svg")
            }
            (Self::PdfFitWidth, true) => {
                include_bytes!("../assets/domain-icons-small/pdf-fit-width.svg")
            }
            (Self::PdfFitWidth, false) => {
                include_bytes!("../assets/domain-icons/pdf-fit-width.svg")
            }
            (Self::PdfActualSize, true) => {
                include_bytes!("../assets/domain-icons-small/pdf-actual-size.svg")
            }
            (Self::PdfActualSize, false) => {
                include_bytes!("../assets/domain-icons/pdf-actual-size.svg")
            }
            (Self::PdfEditText, true) => {
                include_bytes!("../assets/domain-icons-small/pdf-edit-text.svg")
            }
            (Self::PdfEditText, false) => {
                include_bytes!("../assets/domain-icons/pdf-edit-text.svg")
            }
            (Self::PdfClearPage, true) => {
                include_bytes!("../assets/domain-icons-small/pdf-clear-page.svg")
            }
            (Self::PdfClearPage, false) => {
                include_bytes!("../assets/domain-icons/pdf-clear-page.svg")
            }
            (Self::PdfDeletePage, true) => {
                include_bytes!("../assets/domain-icons-small/pdf-delete-page.svg")
            }
            (Self::PdfDeletePage, false) => {
                include_bytes!("../assets/domain-icons/pdf-delete-page.svg")
            }
            (Self::PdfSinglePage, true) => {
                include_bytes!("../assets/domain-icons-small/pdf-single-page.svg")
            }
            (Self::PdfSinglePage, false) => {
                include_bytes!("../assets/domain-icons/pdf-single-page.svg")
            }
            (Self::PdfContinuousPages, true) => {
                include_bytes!("../assets/domain-icons-small/pdf-continuous-pages.svg")
            }
            (Self::PdfContinuousPages, false) => {
                include_bytes!("../assets/domain-icons/pdf-continuous-pages.svg")
            }
            (Self::PdfFileAssociation, true) => {
                include_bytes!("../assets/domain-icons-small/pdf-file-association.svg")
            }
            (Self::PdfFileAssociation, false) => {
                include_bytes!("../assets/domain-icons/pdf-file-association.svg")
            }
            (Self::PdfBookView, true) => {
                include_bytes!("../assets/domain-icons-small/pdf-book-view.svg")
            }
            (Self::PdfBookView, false) => {
                include_bytes!("../assets/domain-icons/pdf-book-view.svg")
            }
            (Self::PdfShortcuts, true) => {
                include_bytes!("../assets/domain-icons-small/pdf-shortcuts.svg")
            }
            (Self::PdfShortcuts, false) => {
                include_bytes!("../assets/domain-icons/pdf-shortcuts.svg")
            }
            (Self::PdfNavigation, true) => {
                include_bytes!("../assets/domain-icons-small/pdf-navigation.svg")
            }
            (Self::PdfNavigation, false) => {
                include_bytes!("../assets/domain-icons/pdf-navigation.svg")
            }
            (Self::PdfManageWatermarks, true) => {
                include_bytes!("../assets/domain-icons-small/pdf-manage-watermarks.svg")
            }
            (Self::PdfManageWatermarks, false) => {
                include_bytes!("../assets/domain-icons/pdf-manage-watermarks.svg")
            }
            (Self::PdfIfcReportExport, true) => {
                include_bytes!("../assets/domain-icons-small/pdf-ifc-report-export.svg")
            }
            (Self::PdfIfcReportExport, false) => {
                include_bytes!("../assets/domain-icons/pdf-ifc-report-export.svg")
            }
            (Self::PdfProofread, true) => {
                include_bytes!("../assets/domain-icons-small/pdf-proofread.svg")
            }
            (Self::PdfProofread, false) => {
                include_bytes!("../assets/domain-icons/pdf-proofread.svg")
            }
            (Self::PdfSplitDocument, true) => {
                include_bytes!("../assets/domain-icons-small/pdf-split-document.svg")
            }
            (Self::PdfSplitDocument, false) => {
                include_bytes!("../assets/domain-icons/pdf-split-document.svg")
            }
        }
    }

    pub fn view<'a>(self, size: u16, color: Color) -> svg::Svg<'a> {
        svg(svg::Handle::from_memory(self.bytes(size)))
            .width(size)
            .height(size)
            .style(move |_, _| svg::Style { color: Some(color) })
    }
}
