//! Embedded assets: GPUI Kit's default component icons plus the Lucide icons
//! zcode's own UI uses. Only referenced SVGs are compiled in.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        ArrowDownToLine,
        ArrowUpFromLine,
        Box,
        Bug,
        ChevronDown,
        Check,
        ChevronRight,
        Circle,
        CircleCheck,
        CircleSlash,
        CircleX,
        Command,
        CornerDownRight,
        Diamond,
        File,
        FileCode,
        FileCog,
        FileImage,
        FileText,
        Folder,
        FolderCode,
        FolderOpen,
        GitBranch,
        Glasses,
        Hammer,
        Hash,
        Info,
        Laptop,
        Layers,
        LayoutTemplate,
        ListOrdered,
        Loader,
        MemoryStick,
        MessageSquareText,
        Package,
        PanelBottom,
        Pause,
        PanelLeft,
        Pencil,
        Play,
        Redo,
        RefreshCw,
        Search,
        Shapes,
        Smartphone,
        Square,
        SquareFunction,
        Tag,
        Tablet,
        Trash,
        TriangleAlert,
        Tv,
        Variable,
        Watch,
        X
    ]
);

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
