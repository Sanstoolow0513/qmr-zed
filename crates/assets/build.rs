use sha2::{Digest, Sha256};
use std::{
    env,
    error::Error,
    fmt::Write as _,
    fs::{self, File},
    io::{BufWriter, Read, Write},
    path::PathBuf,
};

const FONT_FILES: &[&str] = &[
    "MapleMono-NF-CN-Regular.ttf",
    "MapleMono-NF-CN-Italic.ttf",
    "MapleMono-NF-CN-Bold.ttf",
    "MapleMono-NF-CN-BoldItalic.ttf",
];

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_directory =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("CARGO_MANIFEST_DIR is not set")?);
    let output_directory = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is not set")?);
    let font_directory = manifest_directory.join("../../assets/fonts/maple-mono-nf-cn");
    println!("cargo:rerun-if-changed={}", font_directory.display());

    let mut font_paths = Vec::new();
    for entry in fs::read_dir(&font_directory).map_err(|error| {
        format!(
            "reading font directory {}: {error}",
            font_directory.display()
        )
    })? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "ttf") {
            let file_name = path
                .file_name()
                .and_then(|file_name| file_name.to_str())
                .ok_or_else(|| format!("font filename is not UTF-8: {}", path.display()))?;
            let order = FONT_FILES
                .iter()
                .position(|expected| *expected == file_name)
                .ok_or_else(|| format!("unexpected Maple font file: {}", path.display()))?;
            font_paths.push((order, path));
        }
    }
    if font_paths.len() != FONT_FILES.len() {
        return Err(format!(
            "expected {} Maple font files in {}, found {}",
            FONT_FILES.len(),
            font_directory.display(),
            font_paths.len()
        )
        .into());
    }
    font_paths.sort_by_key(|(order, _)| *order);

    let output_path = output_directory.join("maple-fonts.zst");
    let output = BufWriter::new(
        File::create(&output_path)
            .map_err(|error| format!("creating font bundle {}: {error}", output_path.display()))?,
    );
    let mut encoder = zstd::stream::Encoder::new(output, 12)?;
    encoder.window_log(26)?;
    encoder.long_distance_matching(true)?;
    encoder.include_checksum(true)?;

    let mut generated = String::from("static FONT_ENTRIES: &[FontEntry] = &[\n");
    let mut font_offset = 0usize;
    let mut buffer = [0u8; 65536];
    for (order, path) in font_paths {
        println!("cargo:rerun-if-changed={}", path.display());
        let mut file = File::open(&path)
            .map_err(|error| format!("opening font {}: {error}", path.display()))?;
        let mut hash = Sha256::new();
        let mut font_end = font_offset;
        loop {
            let length = file
                .read(&mut buffer)
                .map_err(|error| format!("reading font {}: {error}", path.display()))?;
            if length == 0 {
                break;
            }
            let bytes = &buffer[..length];
            hash.update(bytes);
            encoder
                .write_all(bytes)
                .map_err(|error| format!("compressing font {}: {error}", path.display()))?;
            font_end = font_end
                .checked_add(length)
                .ok_or("combined Maple font size exceeds usize")?;
        }
        let hash: [u8; 32] = hash.finalize().into();
        let asset_path = format!("fonts/maple-mono-nf-cn/{}", FONT_FILES[order]);
        writeln!(
            generated,
            "    FontEntry {{ path: {asset_path:?}, range: {font_offset}..{font_end}, hash: {hash:?} }},"
        )?;
        font_offset = font_end;
    }
    encoder.finish()?.flush()?;
    writeln!(
        generated,
        "];\nconst FONT_DATA_SIZE: usize = {font_offset};"
    )?;
    let manifest_path = output_directory.join("maple-fonts.rs");
    fs::write(&manifest_path, generated).map_err(|error| {
        format!(
            "writing generated font manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    Ok(())
}
