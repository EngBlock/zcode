//! Embedded assets: GPUI Kit's default component icons plus the Lucide icons
//! zcode's own UI uses. Only referenced SVGs are compiled in.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        AppWindow,
        ArrowDown,
        ArrowDownToLine,
        ArrowUp,
        ArrowUpDown,
        ArrowUpFromLine,
        AtSign,
        Bookmark,
        BookmarkPlus,
        Bot,
        Box,
        Brain,
        Bug,
        ChevronDown,
        Check,
        ChevronRight,
        Circle,
        CircleCheck,
        CircleDashed,
        CircleSlash,
        CircleX,
        Columns2,
        Command,
        CornerDownRight,
        Crosshair,
        Diamond,
        Download,
        File,
        FileCode,
        FileCog,
        FileImage,
        FileText,
        FlaskConical,
        Folder,
        FolderCode,
        FolderOpen,
        GitBranch,
        GitBranchPlus,
        GitCommitHorizontal,
        GitCompare,
        Glasses,
        Hammer,
        Hash,
        Info,
        Laptop,
        Layers,
        LayoutTemplate,
        Library,
        ListChecks,
        ListOrdered,
        Loader,
        Lock,
        MemoryStick,
        MessageSquareText,
        Package,
        PanelBottom,
        Pause,
        PanelLeft,
        Pencil,
        Play,
        Puzzle,
        Redo,
        RefreshCw,
        Rows2,
        Search,
        SendHorizontal,
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
        Wrench,
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
