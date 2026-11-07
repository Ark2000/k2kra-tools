//! Reading and writing real user files without disturbing them: encoding, BOM
//! and line endings survive a save, and a write can never leave a half-written file.

use base64::{engine::general_purpose::STANDARD, Engine};
use encoding_rs::{Encoding as Legacy, UTF_16BE, UTF_16LE};
use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, Write};
use std::path::Path;
use std::time::SystemTime;

const MAX_IMAGE_SIZE: u64 = 20 * 1024 * 1024;
/// Above this a file opens read-only: a textarea gets sluggish.
pub const MAX_EDITABLE: u64 = 5 * 1024 * 1024;
/// Above this a file is refused outright.
pub const MAX_OPENABLE: u64 = 20 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Encoding {
    Utf8 { bom: bool },
    Utf16Le,
    Utf16Be,
    Legacy(&'static Legacy),
}

impl Encoding {
    pub fn label(self) -> String {
        match self {
            Encoding::Utf8 { bom: false } => "UTF-8".into(),
            Encoding::Utf8 { bom: true } => "UTF-8 BOM".into(),
            Encoding::Utf16Le => "UTF-16 LE".into(),
            Encoding::Utf16Be => "UTF-16 BE".into(),
            Encoding::Legacy(e) => e.name().into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Eol {
    Lf,
    Crlf,
    Cr,
}

impl Eol {
    pub fn label(self) -> &'static str {
        match self {
            Eol::Lf => "LF",
            Eol::Crlf => "CRLF",
            Eol::Cr => "CR",
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Decoded {
    /// Text with every line ending normalised to `\n`.
    pub text: String,
    pub encoding: Encoding,
    pub eol: Eol,
    pub mixed_eol: bool,
    /// False when saving unchanged text would not reproduce the original bytes.
    pub lossless: bool,
}

pub fn decode(bytes: &[u8]) -> Result<Decoded, String> {
    const BINARY: &str = "This looks like a binary file, not text.";

    let (raw, encoding, lossless) = if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        let text = std::str::from_utf8(rest).map_err(|_| BINARY)?;
        (text.to_owned(), Encoding::Utf8 { bom: true }, true)
    } else if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE") {
        (utf16(rest, UTF_16LE).ok_or(BINARY)?, Encoding::Utf16Le, true)
    } else if let Some(rest) = bytes.strip_prefix(b"\xFE\xFF") {
        (utf16(rest, UTF_16BE).ok_or(BINARY)?, Encoding::Utf16Be, true)
    } else if bytes.contains(&0) {
        return Err(BINARY.into());
    } else if let Ok(text) = std::str::from_utf8(bytes) {
        (text.to_owned(), Encoding::Utf8 { bom: false }, true)
    } else {
        let mut detector = chardetng::EncodingDetector::new();
        detector.feed(bytes, true);
        let guess = detector.guess(None, false);
        let (text, had_errors) = guess.decode_without_bom_handling(bytes);
        (text.into_owned(), Encoding::Legacy(guess), !had_errors)
    };

    let (eol, mixed_eol) = detect_eol(&raw);
    let text = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut decoded = Decoded { text, encoding, eol, mixed_eol, lossless };
    if decoded.lossless && !mixed_eol {
        decoded.lossless = encode(&decoded.text, encoding, eol).is_ok_and(|b| b == bytes);
    }
    Ok(decoded)
}

fn utf16(bytes: &[u8], encoding: &'static Legacy) -> Option<String> {
    if bytes.len() % 2 != 0 {
        return None;
    }
    let (text, had_errors) = encoding.decode_without_bom_handling(bytes);
    (!had_errors).then(|| text.into_owned())
}

fn detect_eol(text: &str) -> (Eol, bool) {
    let (mut crlf, mut lf, mut cr) = (0usize, 0usize, 0usize);
    let bytes = text.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        match b {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => crlf += 1,
            b'\r' => cr += 1,
            b'\n' if i > 0 && bytes[i - 1] == b'\r' => {}
            b'\n' => lf += 1,
            _ => {}
        }
    }
    let kinds = [crlf, lf, cr].iter().filter(|n| **n > 0).count();
    let eol = if crlf >= lf && crlf >= cr && crlf > 0 {
        Eol::Crlf
    } else if cr > lf {
        Eol::Cr
    } else {
        Eol::Lf
    };
    (eol, kinds > 1)
}

/// Turns editor text back into the file's original form. Fails rather than
/// silently substituting characters the file's encoding cannot represent.
pub fn encode(text: &str, encoding: Encoding, eol: Eol) -> Result<Vec<u8>, String> {
    let text = match eol {
        Eol::Lf => text.to_owned(),
        Eol::Crlf => text.replace('\n', "\r\n"),
        Eol::Cr => text.replace('\n', "\r"),
    };
    Ok(match encoding {
        Encoding::Utf8 { bom } => {
            let mut out = Vec::with_capacity(text.len() + 3);
            if bom {
                out.extend_from_slice(b"\xEF\xBB\xBF");
            }
            out.extend_from_slice(text.as_bytes());
            out
        }
        Encoding::Utf16Le => {
            let mut out = vec![0xFF, 0xFE];
            text.encode_utf16().for_each(|u| out.extend_from_slice(&u.to_le_bytes()));
            out
        }
        Encoding::Utf16Be => {
            let mut out = vec![0xFE, 0xFF];
            text.encode_utf16().for_each(|u| out.extend_from_slice(&u.to_be_bytes()));
            out
        }
        Encoding::Legacy(e) => {
            let (bytes, used, had_errors) = e.encode(&text);
            if had_errors || used != e {
                return Err(format!("Some characters cannot be saved as {}.", e.name()));
            }
            bytes.into_owned()
        }
    })
}

/// Identifies one on-disk revision of a file. `None` means it does not exist.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

pub fn stamp(path: &Path) -> io::Result<Option<Stamp>> {
    match fs::metadata(path) {
        Ok(m) => Ok(Some(Stamp { modified: m.modified().ok(), len: m.len() })),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn digest(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// The open file, as of the last time it was read or written.
pub struct Doc {
    pub encoding: Encoding,
    pub eol: Eol,
    pub stamp: Option<Stamp>,
    /// Sync clients often rewrite a file or touch its timestamp without
    /// changing it, so a new stamp alone proves nothing.
    pub digest: Option<u64>,
    pub readonly: bool,
}

#[derive(Debug, PartialEq)]
pub enum Disk {
    Same,
    Changed,
    Deleted,
}

/// Compares the file with what `doc` last saw, by content when the stamp differs.
pub fn refresh(path: &Path, doc: &mut Doc) -> io::Result<Disk> {
    let current = stamp(path)?;
    if current == doc.stamp {
        return Ok(Disk::Same);
    }
    if current.is_none() {
        return Ok(Disk::Deleted);
    }
    if doc.digest != Some(digest(&fs::read(path)?)) {
        return Ok(Disk::Changed);
    }
    doc.stamp = current;
    Ok(Disk::Same)
}

pub struct Loaded {
    pub doc: Doc,
    pub text: String,
    pub notice: Option<String>,
}

pub fn load(path: &Path) -> Result<Loaded, String> {
    let text_err = |e: io::Error| e.to_string();
    let before = stamp(path).map_err(text_err)?;
    let Some(meta) = before.map(|_| fs::metadata(path)).transpose().map_err(text_err)? else {
        // Not there yet: nothing is created until the first edit is saved.
        let doc = Doc {
            encoding: Encoding::Utf8 { bom: false },
            eol: Eol::Lf,
            stamp: None,
            digest: None,
            readonly: false,
        };
        return Ok(Loaded { doc, text: String::new(), notice: None });
    };
    if meta.is_dir() {
        return Err("This is a folder, not a file.".into());
    }
    if meta.len() > MAX_OPENABLE {
        return Err(format!("This file is too large (limit {} MB).", MAX_OPENABLE / 1024 / 1024));
    }

    let bytes = fs::read(path).map_err(text_err)?;
    let decoded = decode(&bytes)?;
    let (readonly, notice) = if meta.permissions().readonly() {
        (true, Some("Read-only file: changes cannot be saved.".to_owned()))
    } else if meta.len() > MAX_EDITABLE {
        (true, Some(format!("Larger than {} MB: opened read-only.", MAX_EDITABLE / 1024 / 1024)))
    } else if !decoded.mixed_eol && !decoded.lossless {
        let name = decoded.encoding.label();
        (true, Some(format!("Encoding detected as {name}, but not with certainty: opened read-only.")))
    } else if decoded.mixed_eol {
        (false, Some(format!("Mixed line endings: saving will use {} throughout.", decoded.eol.label())))
    } else {
        (false, None)
    };
    let doc = Doc {
        encoding: decoded.encoding,
        eol: decoded.eol,
        stamp: before,
        digest: Some(digest(&bytes)),
        readonly,
    };
    Ok(Loaded { doc, text: decoded.text, notice })
}

#[derive(Debug, PartialEq, Serialize)]
pub struct SaveError {
    /// One of `conflict`, `readonly`, `encoding`, `io`.
    pub kind: &'static str,
    pub message: String,
}

fn fail(kind: &'static str, message: impl ToString) -> SaveError {
    SaveError { kind, message: message.to_string() }
}

/// Unless `force` is set, refuses to overwrite a file that changed on disk
/// since this `Doc` last saw it.
pub fn save(path: &Path, doc: &mut Doc, text: &str, force: bool) -> Result<(), SaveError> {
    if doc.readonly {
        return Err(fail("readonly", "This file is read-only."));
    }
    if !force && refresh(path, doc).map_err(|e| fail("io", e))? != Disk::Same {
        return Err(fail("conflict", "The file changed on disk."));
    }
    let bytes = encode(text, doc.encoding, doc.eol).map_err(|e| fail("encoding", e))?;
    write_atomic(path, &bytes).map_err(|e| fail("io", e))?;
    doc.stamp = stamp(path).map_err(|e| fail("io", e))?;
    doc.digest = Some(digest(&bytes));
    Ok(())
}

/// Writes a sibling temp file, then swaps it into place.
fn image_type(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_string_lossy().to_lowercase();
    Some(match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

/// Reads an image as a data URL. Network paths are refused: reading one would
/// make Windows contact a server named by the file, as a remote image would.
pub fn image_url(dir: &Path, src: &str) -> Result<String, String> {
    let separator = |c: char| c == '\\' || c == '/';
    if src.chars().take(2).filter(|c| separator(*c)).count() == 2 {
        return Err("Network images are not loaded.".into());
    }
    let path = dir.join(src);
    let mime = image_type(&path).ok_or("Not an image.")?;
    let size = fs::metadata(&path).map_err(|e| e.to_string())?.len();
    if size > MAX_IMAGE_SIZE {
        return Err("Image is too large.".into());
    }
    let bytes = fs::read(&path).map_err(|e| e.to_string())?;
    Ok(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let name = path.file_name().ok_or(io::ErrorKind::InvalidInput)?.to_string_lossy();
    let tmp = dir.join(format!(".{name}.{}.sticky-tmp", std::process::id()));

    let write = || -> io::Result<()> {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()
    };
    let result = write().and_then(|()| swap_in(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// `ReplaceFileW` keeps the target's ACL and attributes, which a rename would lose.
#[cfg(windows)]
fn swap_in(tmp: &Path, path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null;
    use windows_sys::Win32::Storage::FileSystem::{
        ReplaceFileW, REPLACEFILE_IGNORE_ACL_ERRORS, REPLACEFILE_IGNORE_MERGE_ERRORS,
    };

    if !path.exists() {
        return fs::rename(tmp, path);
    }
    let wide = |p: &Path| p.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>();
    let (target, source) = (wide(path), wide(tmp));
    let flags = REPLACEFILE_IGNORE_MERGE_ERRORS | REPLACEFILE_IGNORE_ACL_ERRORS;
    // SAFETY: both paths are NUL-terminated and outlive the call; the optional
    // pointers are documented as nullable.
    let ok = unsafe {
        ReplaceFileW(target.as_ptr(), source.as_ptr(), null(), flags, null(), null())
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn swap_in(tmp: &Path, path: &Path) -> io::Result<()> {
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_resolve_against_the_note_folder() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("assets")).unwrap();
        fs::write(dir.path().join("assets/a b.PNG"), [1, 2, 3]).unwrap();
        let url = image_url(dir.path(), "assets/a b.PNG").unwrap();
        assert_eq!(url, "data:image/png;base64,AQID");
    }

    #[test]
    fn images_refuse_network_paths_and_other_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("secret.txt"), "x").unwrap();
        for src in [r"\\server\share\a.png", "//server/share/a.png", r"\/server\a.png"] {
            assert_eq!(image_url(dir.path(), src).unwrap_err(), "Network images are not loaded.");
        }
        assert_eq!(image_url(dir.path(), "secret.txt").unwrap_err(), "Not an image.");
    }
    use encoding_rs::{GBK, SHIFT_JIS};

    fn roundtrip(bytes: &[u8]) -> Decoded {
        let d = decode(bytes).unwrap();
        assert_eq!(encode(&d.text, d.encoding, d.eol).unwrap(), bytes);
        assert!(d.lossless);
        d
    }

    #[test]
    fn utf8_with_and_without_bom() {
        assert_eq!(roundtrip("héllo\n".as_bytes()).encoding, Encoding::Utf8 { bom: false });
        assert_eq!(roundtrip(b"\xEF\xBB\xBFhi\n").encoding, Encoding::Utf8 { bom: true });
    }

    #[test]
    fn utf16_both_orders() {
        let le = [&[0xFF, 0xFE][..], &[b'h', 0, b'\r', 0, b'\n', 0]].concat();
        let d = roundtrip(&le);
        assert_eq!((d.encoding, d.eol), (Encoding::Utf16Le, Eol::Crlf));
        assert_eq!(d.text, "h\n");
        assert_eq!(roundtrip(&[0xFE, 0xFF, 0, b'h']).encoding, Encoding::Utf16Be);
    }

    #[test]
    fn gbk_is_detected_even_when_short() {
        for text in ["你好世界", "今天下午三点开会，记得带上项目计划书。\r\n第二行内容。\r\n"] {
            let (bytes, _, _) = GBK.encode(text);
            let d = roundtrip(&bytes);
            assert_eq!(d.encoding, Encoding::Legacy(GBK), "{text}");
        }
    }

    #[test]
    fn line_endings_and_missing_trailing_newline_survive() {
        assert_eq!(roundtrip(b"a\r\nb\r\nc").eol, Eol::Crlf);
        assert_eq!(roundtrip(b"a\nb\n").eol, Eol::Lf);
        assert_eq!(roundtrip(b"a\rb\r").eol, Eol::Cr);
        assert_eq!(roundtrip(b"").eol, Eol::Lf);
    }

    #[test]
    fn mixed_line_endings_are_flagged() {
        let d = decode(b"a\r\nb\r\nc\n").unwrap();
        assert_eq!((d.eol, d.mixed_eol, d.text.as_str()), (Eol::Crlf, true, "a\nb\nc\n"));
    }

    #[test]
    fn binary_is_rejected() {
        assert!(decode(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").is_err());
        assert!(decode(b"\xFF\xFEodd").is_err());
    }

    #[test]
    fn unmappable_characters_fail_instead_of_being_replaced() {
        assert!(encode("😀", Encoding::Legacy(GBK), Eol::Lf).is_err());
        assert!(encode("中", Encoding::Legacy(SHIFT_JIS), Eol::Lf).is_ok());
        assert!(encode("한", Encoding::Legacy(SHIFT_JIS), Eol::Lf).is_err());
    }

    #[test]
    fn save_refuses_external_changes_unless_forced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        fs::write(&path, "one\r\n").unwrap();

        let mut doc = load(&path).unwrap().doc;
        save(&path, &mut doc, "two\n", false).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two\r\n");

        fs::write(&path, "someone else's longer edit").unwrap();
        assert_eq!(save(&path, &mut doc, "three\n", false).unwrap_err().kind, "conflict");
        assert_eq!(fs::read(&path).unwrap(), b"someone else's longer edit");

        save(&path, &mut doc, "three\n", true).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"three\r\n");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1, "temp file left behind");
    }

    #[test]
    fn rewrite_with_identical_content_is_not_a_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synced.md");
        fs::write(&path, "one\n").unwrap();
        let mut doc = load(&path).unwrap().doc;
        save(&path, &mut doc, "two\n", false).unwrap();

        // What a sync client does after uploading: same bytes, new timestamp.
        let later = SystemTime::now() + std::time::Duration::from_secs(60);
        fs::File::options().write(true).open(&path).unwrap().set_modified(later).unwrap();

        assert_eq!(refresh(&path, &mut doc).unwrap(), Disk::Same);
        save(&path, &mut doc, "three\n", false).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"three\n");

        fs::write(&path, "other\n").unwrap();
        assert_eq!(refresh(&path, &mut doc).unwrap(), Disk::Changed);
        fs::remove_file(&path).unwrap();
        assert_eq!(refresh(&path, &mut doc).unwrap(), Disk::Deleted);
    }

    #[test]
    fn missing_file_is_created_only_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.md");
        let mut doc = load(&path).unwrap().doc;
        assert!(!path.exists());
        save(&path, &mut doc, "# hi\n", false).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"# hi\n");
    }

    #[test]
    fn readonly_and_oversized_files_open_readonly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        fs::write(&path, vec![b'a'; MAX_EDITABLE as usize + 1]).unwrap();
        let mut doc = load(&path).unwrap().doc;
        assert!(doc.readonly);
        assert_eq!(save(&path, &mut doc, "x", true).unwrap_err().kind, "readonly");
    }
}
