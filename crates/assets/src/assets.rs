// This crate was essentially pulled out verbatim from main `zed` crate to avoid having to run RustEmbed macro whenever zed has to be rebuilt. It saves a second or two on an incremental build.

use anyhow::Context as _;
use gpui::{App, AssetSource, Result, SharedString};
use std::borrow::Cow;
use util::ResultExt;

pub struct Assets;

// Release builds embed the assets; dev builds read them from the checkout at
// runtime so edits show up on the next launch without a rebuild and no
// build-time path is baked in (which corgi's sandbox rejects). See
// `util::fs_embed!`.
util::fs_embed! {
    struct EmbeddedAssets,
    crate_relative = "../../assets",
    root_relative = "assets",
    include = [
        "fonts/**/*",
        "icons/**/*",
        "images/**/*",
        "themes/**/*",
        "sounds/**/*",
        "prompts/**/*",
        "*.md",
    ],
    exclude = ["themes/src/*", "*.DS_Store", "fonts/maple-mono-nf-cn/*.ttf"],
}

#[cfg(debug_assertions)]
util::fs_embed! {
    struct DevelopmentFonts,
    crate_relative = "../../assets",
    root_relative = "assets",
    include = ["fonts/maple-mono-nf-cn/*.ttf"],
}

impl Assets {
    fn load_file(path: &str) -> Result<Option<util::__rust_embed::EmbeddedFile>> {
        let path = if path.contains('\\') {
            Cow::Owned(path.replace('\\', "/"))
        } else {
            Cow::Borrowed(path)
        };
        let path = path.as_ref();
        #[cfg(not(debug_assertions))]
        if let Some(font) = bundled_fonts::load(path)? {
            return Ok(Some(font));
        }

        #[cfg(debug_assertions)]
        if let Some(font) = DevelopmentFonts::get(path) {
            return Ok(Some(font));
        }

        Ok(EmbeddedAssets::get(path))
    }

    pub fn get(path: &str) -> Option<util::__rust_embed::EmbeddedFile> {
        Self::load_file(path)
            .with_context(|| format!("loading asset at path {path:?}"))
            .log_err()
            .flatten()
    }

    pub fn iter() -> impl Iterator<Item = Cow<'static, str>> {
        #[cfg(not(debug_assertions))]
        let fonts = bundled_fonts::paths().map(Cow::Borrowed);
        #[cfg(debug_assertions)]
        let fonts = DevelopmentFonts::iter();

        EmbeddedAssets::iter().chain(fonts)
    }
}

impl util::__rust_embed::RustEmbed for Assets {
    fn get(path: &str) -> Option<util::__rust_embed::EmbeddedFile> {
        Self::get(path)
    }

    fn iter() -> impl Iterator<Item = Cow<'static, str>> {
        Self::iter()
    }
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Self::load_file(path)
            .map(|file| file.map(|file| file.data))
            .with_context(|| format!("loading asset at path {path:?}"))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter(|file_path| file_path.starts_with(path))
            .map(Into::into)
            .collect())
    }
}

#[cfg(any(not(debug_assertions), test))]
mod bundled_fonts {
    use anyhow::{Context as _, Result, anyhow, ensure};
    use std::{borrow::Cow, io::Read as _, ops::Range, sync::LazyLock};
    use util::__rust_embed::{EmbeddedFile, Metadata};

    struct FontEntry {
        path: &'static str,
        range: Range<usize>,
        hash: [u8; 32],
    }

    include!(concat!(env!("OUT_DIR"), "/maple-fonts.rs"));

    const COMPRESSED_FONTS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/maple-fonts.zst"));

    static FONT_DATA: LazyLock<Result<Vec<u8>>> = LazyLock::new(|| {
        let mut decoder = zstd::stream::Decoder::new(COMPRESSED_FONTS)?;
        decoder.window_log_max(26)?;
        let mut data = Vec::with_capacity(FONT_DATA_SIZE);
        decoder
            .take(FONT_DATA_SIZE as u64 + 1)
            .read_to_end(&mut data)
            .context("decompressing bundled fonts")?;
        ensure!(data.len() == FONT_DATA_SIZE, "invalid font bundle size");
        Ok(data)
    });

    pub(super) fn paths() -> impl Iterator<Item = &'static str> {
        FONT_ENTRIES.iter().map(|font| font.path)
    }

    pub(super) fn load(path: &str) -> Result<Option<EmbeddedFile>> {
        let Some(font) = FONT_ENTRIES.iter().find(|font| font.path == path) else {
            return Ok(None);
        };
        let data = FONT_DATA
            .as_ref()
            .map_err(|error| anyhow!("{error:#}"))?
            .get(font.range.clone())
            .context("invalid bundled font range")?;
        Ok(Some(EmbeddedFile {
            data: Cow::Borrowed(data),
            metadata: Metadata::__rust_embed_new(font.hash, None, None),
        }))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use gpui::AssetSource as _;
        use sha2::{Digest as _, Sha256};
        use std::{collections::HashSet, fs, path::Path};

        #[test]
        fn bundled_fonts_preserve_every_source_face() -> Result<()> {
            let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
            let source_paths = fs::read_dir(assets.join("fonts/maple-mono-nf-cn"))?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<std::io::Result<Vec<_>>>()?;
            let source_names: HashSet<_> = source_paths
                .iter()
                .filter(|path| path.extension().is_some_and(|extension| extension == "ttf"))
                .map(|path| {
                    path.strip_prefix(&assets)
                        .map(|path| path.to_string_lossy().replace('\\', "/"))
                })
                .collect::<std::result::Result<_, _>>()?;
            let bundled_names: HashSet<_> = paths().map(str::to_owned).collect();
            assert_eq!(bundled_names, source_names);
            assert_eq!(FONT_ENTRIES.len(), 4);

            for path in paths() {
                let original = fs::read(assets.join(path))?;
                let bundled = load(path)?.context("missing bundled font")?;
                assert_eq!(bundled.data.as_ref(), original, "{path}");
                assert_eq!(
                    bundled.metadata.sha256_hash(),
                    <[u8; 32]>::from(Sha256::digest(&original))
                );
            }
            assert!(load("fonts/maple-mono-nf-cn/missing.ttf")?.is_none());
            assert!(COMPRESSED_FONTS.len() < 40 * 1024 * 1024);
            Ok(())
        }

        #[test]
        fn public_assets_keep_font_paths_and_hashes() -> Result<()> {
            let listed = crate::Assets.list("fonts/maple-mono-nf-cn/")?;
            for path in paths() {
                assert!(
                    listed
                        .iter()
                        .any(|listed_path| listed_path.as_ref() == path)
                );
                let file = crate::Assets::get(path).context("font missing from assets")?;
                let packed = load(path)?.context("font missing from bundle")?;
                assert_eq!(file.data, packed.data, "{path}");
                assert_eq!(file.metadata.sha256_hash(), packed.metadata.sha256_hash());
                let windows_path = path.replace('/', "\\");
                let windows_file = crate::Assets::get(&windows_path)
                    .context("font missing from assets with Windows separators")?;
                assert_eq!(windows_file.data, file.data);
                assert_eq!(
                    windows_file.metadata.sha256_hash(),
                    file.metadata.sha256_hash()
                );
            }
            assert!(crate::Assets::get("fonts/maple-mono-nf-cn/LICENSE.txt").is_some());
            assert!(crate::Assets::get("fonts/lilex/Lilex-Regular.ttf").is_some());
            assert!(crate::Assets::get("icons/file.svg").is_some());
            Ok(())
        }
    }
}

impl Assets {
    /// Populate the [`TextSystem`] of the given [`AppContext`] with all `.ttf` fonts in the `fonts` directory.
    pub fn load_fonts(&self, cx: &App) -> anyhow::Result<()> {
        let font_paths = self.list("fonts")?;
        let mut embedded_fonts = Vec::new();
        for font_path in font_paths {
            if font_path.ends_with(".ttf") {
                let font_bytes = cx
                    .asset_source()
                    .load(&font_path)?
                    .expect("Assets should never return None");
                embedded_fonts.push(font_bytes);
            }
        }

        cx.text_system().add_fonts(embedded_fonts)
    }

    pub fn load_test_fonts(&self, cx: &App) {
        cx.text_system()
            .add_fonts(vec![
                self.load("fonts/lilex/Lilex-Regular.ttf").unwrap().unwrap(),
            ])
            .unwrap()
    }
}
