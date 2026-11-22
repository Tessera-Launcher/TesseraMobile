//! Normalize downloaded jars: drop signatures and sealed-package attributes.
//!
//! Ported from the desktop build's `jar::normalize`. aria2 fetches the raw
//! Mojang/Fabric jars, which are signed and sometimes sealed. A transformer
//! (Fabric, Mixin, a client patch) then rewrites classes, and the JVM refuses a
//! rewritten class from a signed jar; a sealed package also rejects classes
//! added beside it. So every `.jar` in the download plan is rewritten in place
//! after aria2 and verification, exactly like the desktop install.
//!
//! Signature entries (`META-INF/*.SF`, `*.RSA`, `*.DSA`, `*.EC`) are dropped;
//! the manifest keeps everything except its `Sealed:` lines. A jar that is
//! neither signed nor sealed is left untouched.

use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::Path;

/// Strip JAR signatures and `Sealed:` attributes from `path`, in place.
///
/// Returns `true` when the file was rewritten.
pub fn normalize(path: &Path) -> Result<bool, String> {
    let archive = zip::ZipArchive::new(BufReader::new(open(path)?))
        .map_err(|error| format!("无法解析 {}: {error}", path.display()))?;
    if !needs_normalizing(archive)? {
        return Ok(false);
    }
    rewrite(path)
}

fn needs_normalizing(mut archive: zip::ZipArchive<BufReader<File>>) -> Result<bool, String> {
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("无法读取 jar 条目: {error}"))?;
        let name = entry
            .name()
            .map_err(|error| format!("无法读取 jar 条目名: {error}"))?
            .into_owned();
        if is_signature_entry(&name) {
            return Ok(true);
        }
        if name.eq_ignore_ascii_case("META-INF/MANIFEST.MF") {
            let mut text = String::new();
            entry
                .read_to_string(&mut text)
                .map_err(|error| format!("无法读取 {name}: {error}"))?;
            if seals_a_package(&text) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn rewrite(path: &Path) -> Result<bool, String> {
    let tmp = path.with_extension("normalizing");
    let _ = fs::remove_file(&tmp);
    let result = write_normalized(path, &tmp).and_then(|()| publish(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map(|()| true)
}

fn write_normalized(source: &Path, tmp: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(BufReader::new(open(source)?))
        .map_err(|error| format!("无法解析 {}: {error}", source.display()))?;
    let output =
        File::create(tmp).map_err(|error| format!("无法写入 {}: {error}", tmp.display()))?;
    let mut writer = zip::ZipWriter::new(output);
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("无法读取 jar 条目: {error}"))?;
        let name = entry
            .name()
            .map_err(|error| format!("无法读取 jar 条目名: {error}"))?
            .into_owned();
        if is_signature_entry(&name) {
            continue;
        }
        let options =
            zip::write::SimpleFileOptions::default().compression_method(entry.compression());
        if name.eq_ignore_ascii_case("META-INF/MANIFEST.MF") {
            let mut text = String::new();
            entry
                .read_to_string(&mut text)
                .map_err(|error| format!("无法读取 {name}: {error}"))?;
            writer
                .start_file(&name, options)
                .map_err(|error| format!("无法写入 jar 条目 {name}: {error}"))?;
            writer
                .write_all(unsealed(&text).as_bytes())
                .map_err(|error| format!("无法写入 jar 条目 {name}: {error}"))?;
            continue;
        }
        if entry.is_dir() {
            writer
                .add_directory(&name, options)
                .map_err(|error| format!("无法写入 jar 目录 {name}: {error}"))?;
        } else {
            writer
                .start_file(&name, options)
                .map_err(|error| format!("无法写入 jar 条目 {name}: {error}"))?;
            io::copy(&mut entry, &mut writer)
                .map_err(|error| format!("无法复制 jar 条目 {name}: {error}"))?;
        }
    }
    writer
        .finish()
        .map_err(|error| format!("无法完成 {}: {error}", tmp.display()))?;
    Ok(())
}

fn publish(tmp: &Path, path: &Path) -> Result<(), String> {
    if fs::rename(tmp, path).is_err() {
        fs::remove_file(path).map_err(|error| format!("无法替换 {}: {error}", path.display()))?;
        fs::rename(tmp, path).map_err(|error| format!("无法重命名 {}: {error}", path.display()))?;
    }
    Ok(())
}

fn open(path: &Path) -> Result<File, String> {
    File::open(path).map_err(|error| format!("无法读取 {}: {error}", path.display()))
}

fn is_signature_entry(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("META-INF/")
        && (upper.ends_with(".SF")
            || upper.ends_with(".RSA")
            || upper.ends_with(".DSA")
            || upper.ends_with(".EC"))
}

fn seals_a_package(manifest: &str) -> bool {
    manifest
        .lines()
        .any(|line| line.trim().to_ascii_lowercase().starts_with("sealed:"))
}

fn unsealed(manifest: &str) -> String {
    let mut out = String::with_capacity(manifest.len());
    for line in manifest.split_inclusive('\n') {
        if line.trim().to_ascii_lowercase().starts_with("sealed:") {
            continue;
        }
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn jar(path: &Path, entries: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, body) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
    }

    fn names(path: &Path) -> Vec<String> {
        let mut archive = zip::ZipArchive::new(BufReader::new(File::open(path).unwrap())).unwrap();
        (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().unwrap().to_string())
            .collect()
    }

    fn manifest(path: &Path) -> String {
        let mut archive = zip::ZipArchive::new(BufReader::new(File::open(path).unwrap())).unwrap();
        let mut entry = archive.by_name("META-INF/MANIFEST.MF").unwrap();
        let mut text = String::new();
        entry.read_to_string(&mut text).unwrap();
        text
    }

    #[test]
    fn a_signed_jar_loses_its_signature_and_sealing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("client.jar");
        jar(
            &path,
            &[
                (
                    "META-INF/MANIFEST.MF",
                    b"Manifest-Version: 1.0\nSealed: true\n\nName: ave.class\nSHA-256-Digest: x\n",
                ),
                ("META-INF/MOJANGCS.SF", b"Signature-Version: 1.0\n"),
                ("META-INF/MOJANGCS.RSA", b"\x30\x82"),
                ("ave.class", b"CAFEBABE"),
            ],
        );

        assert!(normalize(&path).unwrap());

        let left = names(&path);
        assert!(!left.iter().any(|name| name.ends_with(".SF")));
        assert!(!left.iter().any(|name| name.ends_with(".RSA")));
        assert!(left.iter().any(|name| name == "ave.class"));
        let text = manifest(&path);
        assert!(!text.to_ascii_lowercase().contains("sealed:"));
        assert!(text.contains("SHA-256-Digest"));
    }

    #[test]
    fn the_rewrite_keeps_no_backup_and_no_temporary() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("client.jar");
        jar(
            &path,
            &[
                ("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0\n"),
                ("META-INF/MOJANGCS.SF", b"Signature-Version: 1.0\n"),
                ("ave.class", b"CAFEBABE"),
            ],
        );

        assert!(normalize(&path).unwrap());

        let left: Vec<String> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, vec!["client.jar".to_string()], "in place, no copy");
    }

    #[test]
    fn an_unsigned_unsealed_jar_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("library.jar");
        jar(
            &path,
            &[
                ("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0\n"),
                ("a/Thing.class", b"CAFEBABE"),
            ],
        );
        let before = fs::read(&path).unwrap();

        assert!(!normalize(&path).unwrap());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}
