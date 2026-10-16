use std::sync::Arc;
use std::{path::Path, str};

use gpui::{App, SharedString};
use settings_content::FolderIndicator;
use theme::{DirectoryIcons, GlobalTheme, IconColors, IconDefinition, IconTheme, ThemeRegistry};
use util::paths::PathExt;

#[derive(Debug)]
pub struct FileIcons {
    icon_theme: Arc<IconTheme>,
}

/// What a panel draws ahead of a directory's name, in render order.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FolderIndicators {
    pub chevron: Option<SharedString>,
    pub icon: Option<SharedString>,
    pub icon_colors: Option<IconColors>,
}

impl FileIcons {
    pub fn get(cx: &App) -> Self {
        Self {
            icon_theme: GlobalTheme::icon_theme(cx).clone(),
        }
    }

    pub fn get_icon(path: &Path, cx: &App) -> Option<SharedString> {
        Self::get_icon_definition(path, cx).map(|icon| icon.path)
    }

    pub fn get_icon_definition(path: &Path, cx: &App) -> Option<IconDefinition> {
        let this = Self::get(cx);

        let get_icon_from_suffix = |suffix: &str| -> Option<IconDefinition> {
            this.icon_theme
                .file_stems
                .get(suffix)
                .or_else(|| this.icon_theme.file_suffixes.get(suffix))
                .and_then(|typ| this.get_icon_definition_for_type(typ, cx))
        };
        // TODO: Associate a type with the languages and have the file's language
        //       override these associations

        if let Some(typ) = path.file_name().and_then(|typ| typ.to_str()) {
            // check if file name is in suffixes
            // e.g. catch file named `eslint.config.js` instead of `.eslint.config.js`
            let maybe_path = get_icon_from_suffix(typ);
            if maybe_path.is_some() {
                return maybe_path;
            }

            if let Some((stem, mut suffix)) = typ.split_once('.') {
                // check if stem (before first dot) is in stems
                // e.g. catch file named `Dockerfile.prod` or `Podfile.lock`
                let maybe_path = get_icon_from_suffix(stem);
                if maybe_path.is_some() {
                    return maybe_path;
                }

                loop {
                    // check if suffix based on first dot is in suffixes
                    // e.g. consider `module.js` as suffix to angular's module file named `auth.module.js`
                    let maybe_path = get_icon_from_suffix(suffix);
                    if maybe_path.is_some() {
                        return maybe_path;
                    }

                    let Some((_, next_suffix)) = suffix.split_once('.') else {
                        break;
                    };
                    suffix = next_suffix;
                }
            }
        }

        // handle cases where the file extension is made up of multiple important
        // parts (e.g Component.stories.tsx) that refer to an alternative icon style
        if let Some(suffix) = path.multiple_extensions() {
            let maybe_path = get_icon_from_suffix(suffix.as_str());
            if maybe_path.is_some() {
                return maybe_path;
            }
        }

        // primary case: check if the files extension or the hidden file name
        // matches some icon path
        if let Some(suffix) = path.extension_or_hidden_file_name() {
            let maybe_path = get_icon_from_suffix(suffix);
            if maybe_path.is_some() {
                return maybe_path;
            }
        }

        // this _should_ only happen when the file is hidden (has leading '.')
        // and is not a "special" file we have an icon (e.g. not `.eslint.config.js`)
        // that should be caught above. In the remaining cases, we want to check
        // for a normal supported extension e.g. `.data.json` -> `json`
        let extension = path.extension().and_then(|ext| ext.to_str());
        if let Some(extension) = extension {
            let maybe_path = get_icon_from_suffix(extension);
            if maybe_path.is_some() {
                return maybe_path;
            }
        }
        this.get_icon_definition_for_type("default", cx)
    }

    fn default_icon_theme(cx: &App) -> Option<Arc<IconTheme>> {
        let theme_registry = ThemeRegistry::global(cx);
        theme_registry.default_icon_theme().ok()
    }

    pub fn get_icon_for_type(&self, typ: &str, cx: &App) -> Option<SharedString> {
        self.get_icon_definition_for_type(typ, cx)
            .map(|icon| icon.path)
    }

    fn get_icon_definition_for_type(&self, typ: &str, cx: &App) -> Option<IconDefinition> {
        self.icon_theme.file_icons.get(typ).cloned().or_else(|| {
            Self::default_icon_theme(cx)
                .and_then(|icon_theme| icon_theme.file_icons.get(typ).cloned())
        })
    }

    pub fn get_folder_icon(expanded: bool, path: &Path, cx: &App) -> Option<SharedString> {
        Self::get_folder_icon_definition(expanded, path, cx).map(|icon| icon.path)
    }

    pub fn get_folder_icon_definition(
        expanded: bool,
        path: &Path,
        cx: &App,
    ) -> Option<IconDefinition> {
        fn get_folder_icon(
            icon_theme: &Arc<IconTheme>,
            path: &Path,
            expanded: bool,
        ) -> Option<IconDefinition> {
            let name = path.file_name()?.to_str()?.trim();
            if name.is_empty() {
                return None;
            }

            let directory_icons = icon_theme.named_directory_icons.get(name)?;

            FileIcons::directory_icon_definition(directory_icons, expanded)
        }

        get_folder_icon(GlobalTheme::icon_theme(cx), path, expanded)
            .or_else(|| {
                Self::default_icon_theme(cx)
                    .and_then(|icon_theme| get_folder_icon(&icon_theme, path, expanded))
            })
            .or_else(|| {
                // If we can't find a specific folder icon for the folder at the given path, fall back to the generic folder
                // icon.
                Self::get_generic_folder_icon(expanded, cx)
            })
    }

    fn directory_icon_definition(
        directory_icons: &DirectoryIcons,
        expanded: bool,
    ) -> Option<IconDefinition> {
        let path = if expanded {
            directory_icons.expanded.clone()
        } else {
            directory_icons.collapsed.clone()
        }?;
        Some(IconDefinition {
            path,
            colors: directory_icons.colors,
        })
    }

    fn get_generic_folder_icon(expanded: bool, cx: &App) -> Option<IconDefinition> {
        fn get_generic_folder_icon(
            icon_theme: &Arc<IconTheme>,
            expanded: bool,
        ) -> Option<IconDefinition> {
            FileIcons::directory_icon_definition(&icon_theme.directory_icons, expanded)
        }

        get_generic_folder_icon(GlobalTheme::icon_theme(cx), expanded).or_else(|| {
            Self::default_icon_theme(cx)
                .and_then(|icon_theme| get_generic_folder_icon(&icon_theme, expanded))
        })
    }

    pub fn get_chevron_icon(expanded: bool, cx: &App) -> Option<SharedString> {
        fn get_chevron_icon(icon_theme: &Arc<IconTheme>, expanded: bool) -> Option<SharedString> {
            if expanded {
                icon_theme.chevron_icons.expanded.clone()
            } else {
                icon_theme.chevron_icons.collapsed.clone()
            }
        }

        get_chevron_icon(GlobalTheme::icon_theme(cx), expanded).or_else(|| {
            Self::default_icon_theme(cx)
                .and_then(|icon_theme| get_chevron_icon(&icon_theme, expanded))
        })
    }

    /// Resolves what a panel should draw ahead of a directory's name. Shared by every
    /// panel that exposes a `folder_indicator` setting so they stay in agreement.
    pub fn get_folder_indicators(
        indicator: FolderIndicator,
        expanded: bool,
        path: &Path,
        cx: &App,
    ) -> FolderIndicators {
        let chevron = indicator
            .shows_chevron()
            .then(|| Self::get_chevron_icon(expanded, cx))
            .flatten();
        let icon = indicator
            .shows_icon()
            .then(|| Self::get_folder_icon_definition(expanded, path, cx))
            .flatten();

        FolderIndicators {
            chevron,
            icon_colors: icon.as_ref().and_then(|icon| icon.colors),
            icon: icon.map(|icon| icon.path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use theme::{Appearance, CLAUDE_ICON_THEME_NAME};

    fn use_claude_icons(cx: &mut App) {
        theme::init(theme::LoadThemes::JustBase, cx);
        let icons = ThemeRegistry::global(cx)
            .get_icon_theme(CLAUDE_ICON_THEME_NAME)
            .unwrap();
        GlobalTheme::update_icon_theme(cx, icons);
    }

    #[gpui::test]
    fn test_claude_file_associations_and_fallback(cx: &mut App) {
        use_claude_icons(cx);

        for (filename, asset) in [
            ("main.rs", "rust"),
            ("main.ts", "typescript"),
            ("main.py", "python"),
            ("Component.stories.tsx", "react"),
            (".data.json", "json"),
            ("Cargo.toml", "toml"),
            ("README.md", "markdown"),
            (".gitignore", "git"),
            ("archive.tar.gz", "archive"),
            ("unknown.custom", "file"),
        ] {
            let icon = FileIcons::get_icon_definition(Path::new(filename), cx).unwrap();
            assert_eq!(icon.path, format!("icons/file_icons/claude/{asset}.svg"));
            assert!(icon.colors.is_some(), "missing tint for {filename}");
            assert_eq!(
                FileIcons::get_icon(Path::new(filename), cx),
                Some(icon.path)
            );
        }

        let docker = FileIcons::get_icon_definition(Path::new("Dockerfile"), cx).unwrap();
        assert_eq!(docker.path, "icons/file_icons/docker.svg");
        assert!(docker.colors.is_some());
    }

    #[gpui::test]
    fn test_claude_tint_follows_ui_appearance_independently_of_system(cx: &mut App) {
        use_claude_icons(cx);
        let icon = FileIcons::get_icon_definition(Path::new("main.rs"), cx).unwrap();
        let colors = icon.colors.unwrap();
        assert_ne!(colors.light, colors.dark);

        for (system_appearance, ui_appearance, expected) in [
            (Appearance::Light, Appearance::Dark, colors.dark),
            (Appearance::Dark, Appearance::Light, colors.light),
        ] {
            theme::SystemAppearance::global_mut(cx).0 = system_appearance;
            let mut ui_theme = GlobalTheme::theme(cx).as_ref().clone();
            ui_theme.appearance = ui_appearance;
            GlobalTheme::update_theme(cx, Arc::new(ui_theme));

            assert_eq!(
                colors.for_appearance(GlobalTheme::theme(cx).appearance()),
                expected
            );
            assert_eq!(GlobalTheme::icon_theme(cx).name, CLAUDE_ICON_THEME_NAME);
        }
    }

    #[gpui::test]
    fn test_claude_folder_indicators_keep_tint_and_chevron_separate(cx: &mut App) {
        use_claude_icons(cx);
        let path = Path::new("src");
        let collapsed = FileIcons::get_folder_indicators(FolderIndicator::Both, false, path, cx);
        let expanded = FileIcons::get_folder_indicators(FolderIndicator::Both, true, path, cx);
        assert_eq!(
            collapsed.icon.as_deref(),
            Some("icons/file_icons/claude/folder.svg")
        );
        assert_eq!(
            expanded.icon.as_deref(),
            Some("icons/file_icons/claude/folder_open.svg")
        );
        assert_eq!(collapsed.icon_colors, expanded.icon_colors);
        assert!(collapsed.icon_colors.is_some());
        assert_eq!(
            collapsed.chevron.as_deref(),
            Some("icons/file_icons/chevron_right.svg")
        );

        let chevron = FileIcons::get_folder_indicators(FolderIndicator::Chevron, false, path, cx);
        assert!(chevron.icon.is_none());
        assert!(chevron.icon_colors.is_none());
    }

    #[gpui::test]
    fn test_external_icon_theme_preserves_images_and_zed_fallback(cx: &mut App) {
        use_claude_icons(cx);
        let registry = ThemeRegistry::global(cx);
        let root = Path::new("extension");
        registry
            .load_icon_theme(
                &root.join("theme.json"),
                root,
                br#"{
                    "name": "External", "author": "Test",
                    "themes": [{
                        "name": "External", "appearance": "dark",
                        "directory_icons": {"collapsed": "icons/folder.svg"},
                        "named_directory_icons": {"src": {"expanded": "icons/src-open.svg"}},
                        "file_stems": {"Cargo.toml": "cargo"},
                        "file_suffixes": {"special.rs": "special"},
                        "file_icons": {
                            "cargo": {"path": "icons/cargo.svg"},
                            "special": {"path": "icons/special.svg"}
                        }
                    }]
                }"#
                .to_vec(),
            )
            .unwrap();
        GlobalTheme::update_icon_theme(cx, registry.get_icon_theme("External").unwrap());

        for (filename, asset) in [("Cargo.toml", "cargo"), ("name.special.rs", "special")] {
            let icon = FileIcons::get_icon_definition(Path::new(filename), cx).unwrap();
            assert_eq!(
                icon.path.as_ref(),
                root.join(format!("icons/{asset}.svg")).to_string_lossy()
            );
            assert!(icon.colors.is_none());
        }

        let rust = FileIcons::get_icon_definition(Path::new("main.rs"), cx).unwrap();
        assert_eq!(rust.path, "icons/file_icons/rust.svg");
        assert!(rust.colors.is_none());
        let unknown = FileIcons::get_icon_definition(Path::new("unknown.custom"), cx).unwrap();
        assert_eq!(unknown.path, "icons/file_icons/file.svg");
        assert!(unknown.colors.is_none());

        let source = FileIcons::get_folder_icon_definition(true, Path::new("src"), cx).unwrap();
        assert_eq!(
            source.path.as_ref(),
            root.join("icons/src-open.svg").to_string_lossy()
        );
        assert!(source.colors.is_none());
        let folder = FileIcons::get_folder_icon_definition(true, Path::new("other"), cx).unwrap();
        assert_eq!(folder.path, "icons/file_icons/folder_open.svg");
        assert!(folder.colors.is_none());
    }

    #[gpui::test]
    fn test_folder_indicators_per_setting(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            theme::init(theme::LoadThemes::JustBase, cx);
        });

        let path = PathBuf::from("src");

        cx.update(|cx| {
            let icon_only =
                FileIcons::get_folder_indicators(FolderIndicator::Icon, false, &path, cx);
            assert_eq!(icon_only.chevron, None);
            assert_eq!(
                icon_only.icon,
                Some("icons/file_icons/folder.svg".into()),
                "`icon` should draw the folder icon and no chevron"
            );

            let chevron_only =
                FileIcons::get_folder_indicators(FolderIndicator::Chevron, false, &path, cx);
            assert_eq!(
                chevron_only.chevron,
                Some("icons/file_icons/chevron_right.svg".into())
            );
            assert_eq!(
                chevron_only.icon, None,
                "`chevron` should draw the chevron and no folder icon"
            );

            let both = FileIcons::get_folder_indicators(FolderIndicator::Both, false, &path, cx);
            assert_eq!(
                both.chevron,
                Some("icons/file_icons/chevron_right.svg".into())
            );
            assert_eq!(both.icon, Some("icons/file_icons/folder.svg".into()));
        });
    }

    #[gpui::test]
    fn test_folder_indicators_reflect_expanded_state(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            theme::init(theme::LoadThemes::JustBase, cx);
        });

        let path = PathBuf::from("src");

        cx.update(|cx| {
            let collapsed =
                FileIcons::get_folder_indicators(FolderIndicator::Both, false, &path, cx);
            assert_eq!(
                collapsed.chevron,
                Some("icons/file_icons/chevron_right.svg".into())
            );
            assert_eq!(collapsed.icon, Some("icons/file_icons/folder.svg".into()));

            let expanded = FileIcons::get_folder_indicators(FolderIndicator::Both, true, &path, cx);
            assert_eq!(
                expanded.chevron,
                Some("icons/file_icons/chevron_down.svg".into())
            );
            assert_eq!(
                expanded.icon,
                Some("icons/file_icons/folder_open.svg".into())
            );
        });
    }

    #[gpui::test]
    fn test_folder_indicator_default_is_icon(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            theme::init(theme::LoadThemes::JustBase, cx);
        });

        let path = PathBuf::from("src");

        cx.update(|cx| {
            let default =
                FileIcons::get_folder_indicators(FolderIndicator::default(), false, &path, cx);
            assert_eq!(
                default,
                FileIcons::get_folder_indicators(FolderIndicator::Icon, false, &path, cx),
                "the default must stay `icon` so existing users see no change"
            );
        });
    }
}
