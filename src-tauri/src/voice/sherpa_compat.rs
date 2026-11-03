//! sherpa-onnx compatibility install for macOS older than 13.4.
//!
//! The pinned PyPI wheel (sherpa-onnx 1.12.26) bundles onnxruntime 1.23.2, which
//! needs macOS 13.4; its `_sherpa_onnx` module also imports `std::to_chars(double)`
//! from a newer system libc++. On older systems the wheel installs but cannot load,
//! so wake word and voiceprint checks are unavailable. PA then swaps in a build of
//! the same release linked against onnxruntime 1.19.2 (macOS 11.0), produced by
//! `scripts/build-sherpa-compat.sh` and published as a GitHub Release asset.
//!
//! This is a supply-chain entry point: the URL origin and per-architecture SHA256
//! are fixed here, the archive is verified before it is opened, only two named
//! regular files are extracted, and everything is staged inside the venv before
//! being renamed into place. No shell is involved.

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};

const RELEASE_BASE: &str =
    "https://github.com/kevin-ping/pocket-agent/releases/download/sherpa-compat-1.12.26-ort1.19.2/";
const SHERPA_VERSION: &str = "1.12.26";
const PYTHON_VERSION: &str = "3.9";
const MODULE: &str = "_sherpa_onnx.cpython-39-darwin.so";
const ORT_LIB: &str = "libonnxruntime.1.19.2.dylib";
/// Install order matters: the new onnxruntime has a new file name, so writing it
/// first never touches an existing file. The module replacement comes last.
const FILES: [&str; 2] = [ORT_LIB, MODULE];
const MARKER: &str = ".pa-sherpa-compat";
const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 96 * 1024 * 1024;
/// macOS release that runs the stock wheel.
const NATIVE_MIN_MACOS: (u32, u32) = (13, 4);

struct Asset {
    machine: &'static str,
    name: &'static str,
    sha256: &'static str,
}

const ASSETS: [Asset; 2] = [
    Asset {
        machine: "x86_64",
        name: "sherpa-onnx-compat-1.12.26-ort1.19.2-macos11-x86_64-cp39.tar.gz",
        sha256: "4e83b234bf3c6faf3c579c174ed7f3163eaba68bce2c31283b30b7b5310ebb72",
    },
    Asset {
        machine: "arm64",
        name: "sherpa-onnx-compat-1.12.26-ort1.19.2-macos11-arm64-cp39.tar.gz",
        sha256: "99a5573cc56fa04fc31f5df69f8ff29bba428aa947d942660a3c59d4596886ea",
    },
];

/// Reads the importability facts from the venv interpreter without a shell.
/// `-I` keeps the user's environment and cwd out of `sys.path`.
const PROBE_SCRIPT: &str = r#"
import importlib.util, json, platform, sys
r = {"machine": platform.machine(), "python": "%d.%d" % sys.version_info[:2]}
try:
    from importlib.metadata import version
    r["sherpa_version"] = version("sherpa-onnx")
except Exception:
    r["sherpa_version"] = None
spec = importlib.util.find_spec("sherpa_onnx")
locs = list(spec.submodule_search_locations or []) if spec else []
r["lib_dir"] = locs[0] + "/lib" if locs else None
try:
    import sherpa_onnx
    r["ok"], r["error"] = True, ""
except Exception as e:
    r["ok"], r["error"] = False, str(e)
print(json.dumps(r))
"#;

#[derive(serde::Deserialize)]
struct Probe {
    machine: String,
    python: String,
    sherpa_version: Option<String>,
    lib_dir: Option<PathBuf>,
    ok: bool,
    error: String,
}

pub fn macos_version() -> Option<(u32, u32)> {
    let out = Command::new("/usr/sbin/sysctl")
        .args(["-n", "kern.osproductversion"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    parse_version(String::from_utf8_lossy(&out.stdout).trim())
}

fn parse_version(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |m| m.parse().ok())?;
    Some((major, minor))
}

fn probe(python: &Path) -> Result<Probe, String> {
    let out = Command::new(python)
        .args(["-I", "-c", PROBE_SCRIPT])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("probe spawn failed: {e}"))?;
    if !out.status.success() {
        return Err(format!("probe exited {:?}: {}", out.status.code(), String::from_utf8_lossy(&out.stderr).trim()));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("probe output unreadable: {e}"))
}

/// Make sherpa-onnx loadable on macOS older than 13.4. Called after the venv is
/// ready, alongside stt-server startup; errors are logged by the caller and only
/// leave wake word / voiceprint unavailable. Returns true when this call
/// installed the compat build, so a running stt-server must reload it.
pub fn ensure_compat() -> Result<bool, String> {
    let Some(version) = macos_version() else {
        return Err("cannot read the macOS version".into());
    };
    if version >= NATIVE_MIN_MACOS {
        return Ok(false);
    }
    let python = crate::voice::venv::venv_python_path();
    let before = probe(&python)?;
    if before.ok {
        return Ok(false);
    }
    eprintln!("[sherpa-compat] macOS {}.{}: sherpa_onnx does not load: {}", version.0, version.1, before.error);
    if before.python != PYTHON_VERSION {
        return Err(format!("venv Python is {}, compat build targets {PYTHON_VERSION}", before.python));
    }
    if before.sherpa_version.as_deref() != Some(SHERPA_VERSION) {
        return Err(format!("installed sherpa-onnx is {:?}, compat build targets {SHERPA_VERSION}", before.sherpa_version));
    }
    let asset = ASSETS
        .iter()
        .find(|a| a.machine == before.machine)
        .ok_or_else(|| format!("no compat build for {}", before.machine))?;
    let lib_dir = before.lib_dir.ok_or("sherpa_onnx package not found")?;
    let lib_dir = lib_dir.canonicalize().map_err(|e| format!("{}: {e}", lib_dir.display()))?;
    let venv = crate::voice::venv::venv_root()
        .canonicalize()
        .map_err(|e| format!("venv: {e}"))?;
    if !lib_dir.starts_with(&venv) {
        return Err(format!("{} is outside the PA venv", lib_dir.display()));
    }

    eprintln!("[sherpa-compat] installing {}", asset.name);
    install(&lib_dir, asset, curl_download)?;
    let after = probe(&python)?;
    if !after.ok {
        return Err(format!("compat build installed but still does not load: {}", after.error));
    }
    eprintln!("[sherpa-compat] sherpa_onnx loads with {}", asset.name);
    Ok(true)
}

/// Fixed argv, HTTPS only (including redirects to GitHub's asset host), bounded
/// size and time. curl exits non-zero on HTTP errors and partial transfers.
fn curl_download(url: &str, dest: &Path) -> Result<(), String> {
    let out = Command::new("/usr/bin/curl")
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--proto", "=https", "--proto-redir", "=https", "--tlsv1.2"])
        .args(["--max-redirs", "5", "--connect-timeout", "20", "--max-time", "600"])
        .args(["--max-filesize", &MAX_ARCHIVE_BYTES.to_string()])
        .arg("--output")
        .arg(dest)
        .arg(url)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl spawn failed: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("download failed ({:?}): {}", out.status.code(), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// Download, verify and install `asset` into `lib_dir`. The staging directory
/// lives inside `lib_dir` so the final renames stay on one filesystem; it is
/// removed on every exit path. Until the last rename succeeds the original
/// module is untouched.
fn install(
    lib_dir: &Path,
    asset: &Asset,
    download: impl FnOnce(&str, &Path) -> Result<(), String>,
) -> Result<(), String> {
    let staging = tempfile::Builder::new()
        .prefix(".pa-compat-")
        .tempdir_in(lib_dir)
        .map_err(|e| format!("staging dir: {e}"))?;
    let archive = staging.path().join("archive.tar.gz");
    download(&format!("{RELEASE_BASE}{}", asset.name), &archive)?;
    verify_sha256(&archive, asset.sha256)?;
    let files = staging.path().join("files");
    std::fs::create_dir(&files).map_err(|e| format!("staging dir: {e}"))?;
    extract_whitelisted(&archive, &files)?;
    for name in FILES {
        std::fs::rename(files.join(name), lib_dir.join(name))
            .map_err(|e| format!("install {name}: {e}"))?;
    }
    std::fs::write(lib_dir.join(MARKER), format!("{} {}\n", asset.name, asset.sha256))
        .map_err(|e| format!("marker: {e}"))?;
    Ok(())
}

fn verify_sha256(path: &Path, expected: &str) -> Result<(), String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("archive: {e}"))?;
    let size = file.metadata().map_err(|e| format!("archive: {e}"))?.len();
    if size == 0 || size > MAX_ARCHIVE_BYTES {
        return Err(format!("archive size {size} is out of range"));
    }
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| format!("archive: {e}"))?;
    let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if actual != expected {
        return Err(format!("archive checksum mismatch (got {actual})"));
    }
    Ok(())
}

/// Accept exactly the files in `FILES`, each once, as regular files at the
/// archive root. Anything else — links, directories, absolute or `..` paths,
/// unexpected names, oversized entries — rejects the whole archive.
fn extract_whitelisted(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| format!("archive: {e}"))?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut seen = Vec::new();
    for entry in tar.entries().map_err(|e| format!("archive: {e}"))? {
        let mut entry = entry.map_err(|e| format!("archive: {e}"))?;
        let path = entry.path().map_err(|e| format!("archive path: {e}"))?.into_owned();
        let name = match path.components().collect::<Vec<_>>().as_slice() {
            [Component::Normal(n)] => n.to_str().unwrap_or_default().to_string(),
            _ => return Err(format!("archive entry {} is not a plain file name", path.display())),
        };
        if !entry.header().entry_type().is_file() {
            return Err(format!("archive entry {name} is not a regular file"));
        }
        if !FILES.contains(&name.as_str()) || seen.contains(&name) {
            return Err(format!("unexpected archive entry {name}"));
        }
        if entry.size() > MAX_FILE_BYTES {
            return Err(format!("archive entry {name} is too large"));
        }
        let out_path = dest.join(&name);
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&out_path)
            .map_err(|e| format!("extract {name}: {e}"))?;
        let copied = std::io::copy(&mut (&mut entry).take(MAX_FILE_BYTES + 1), &mut out)
            .map_err(|e| format!("extract {name}: {e}"))?;
        if copied == 0 || copied > MAX_FILE_BYTES {
            return Err(format!("archive entry {name} has invalid size"));
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("extract {name}: {e}"))?;
        seen.push(name);
    }
    if seen.len() != FILES.len() {
        return Err("archive is missing required files".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tar_gz(build: impl FnOnce(&mut tar::Builder<Vec<u8>>)) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        build(&mut builder);
        let tar = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &tar).unwrap();
        gz.finish().unwrap()
    }

    fn add_file(b: &mut tar::Builder<Vec<u8>>, name: &str, data: &[u8]) {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        // set_path refuses `..`; write the raw name so traversal can be tested.
        h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
        h.set_cksum();
        b.append(&h, data).unwrap();
    }

    fn good_archive() -> Vec<u8> {
        tar_gz(|b| {
            add_file(b, ORT_LIB, b"new-ort");
            add_file(b, MODULE, b"new-module");
        })
    }

    fn sha_hex(data: &[u8]) -> String {
        Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// A lib dir holding the stock module, plus an asset whose pinned hash is `sha`.
    fn setup(sha: String) -> (tempfile::TempDir, Asset) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(MODULE), b"old-module").unwrap();
        let asset = Asset { machine: "arm64", name: "test.tar.gz", sha256: Box::leak(sha.into_boxed_str()) };
        (dir, asset)
    }

    fn serve(bytes: Vec<u8>) -> impl FnOnce(&str, &Path) -> Result<(), String> {
        move |url, dest| {
            assert!(url.starts_with("https://github.com/kevin-ping/pocket-agent/releases/download/"));
            std::fs::write(dest, bytes).map_err(|e| e.to_string())
        }
    }

    fn assert_untouched(dir: &Path) {
        assert_eq!(std::fs::read(dir.join(MODULE)).unwrap(), b"old-module");
        let left: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from(MODULE)], "staging must be cleaned up");
    }

    #[test]
    fn installs_verified_archive() {
        let bytes = good_archive();
        let (dir, asset) = setup(sha_hex(&bytes));
        install(dir.path(), &asset, serve(bytes)).unwrap();
        assert_eq!(std::fs::read(dir.path().join(MODULE)).unwrap(), b"new-module");
        assert_eq!(std::fs::read(dir.path().join(ORT_LIB)).unwrap(), b"new-ort");
        assert!(dir.path().join(MARKER).is_file());
    }

    #[test]
    fn rejects_checksum_mismatch() {
        let (dir, asset) = setup("0".repeat(64));
        let err = install(dir.path(), &asset, serve(good_archive())).unwrap_err();
        assert!(err.contains("checksum mismatch"), "{err}");
        assert_untouched(dir.path());
    }

    #[test]
    fn rejects_interrupted_download() {
        let bytes = good_archive();
        let (dir, asset) = setup(sha_hex(&bytes));
        let truncated = bytes[..bytes.len() / 2].to_vec();
        assert!(install(dir.path(), &asset, serve(truncated)).is_err());
        assert_untouched(dir.path());

        let (dir, asset) = setup(sha_hex(&bytes));
        let err = install(dir.path(), &asset, |_, _| Err("download failed (Some(18))".into())).unwrap_err();
        assert!(err.contains("download failed"), "{err}");
        assert_untouched(dir.path());
    }

    #[test]
    fn rejects_path_traversal_and_absolute_paths() {
        for name in ["../evil.so", "/tmp/evil.so", "sub/_sherpa_onnx.cpython-39-darwin.so"] {
            let bytes = tar_gz(|b| {
                add_file(b, ORT_LIB, b"new-ort");
                add_file(b, name, b"evil");
            });
            let (dir, asset) = setup(sha_hex(&bytes));
            let err = install(dir.path(), &asset, serve(bytes)).unwrap_err();
            assert!(err.contains("not a plain file name"), "{name}: {err}");
            assert_untouched(dir.path());
        }
    }

    #[test]
    fn rejects_symlinks_extra_and_missing_files() {
        let symlink = tar_gz(|b| {
            add_file(b, ORT_LIB, b"new-ort");
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            b.append_link(&mut h, MODULE, "/usr/lib/libSystem.B.dylib").unwrap();
        });
        let extra = tar_gz(|b| {
            add_file(b, ORT_LIB, b"new-ort");
            add_file(b, MODULE, b"new-module");
            add_file(b, "libextra.dylib", b"extra");
        });
        let duplicate = tar_gz(|b| {
            add_file(b, ORT_LIB, b"new-ort");
            add_file(b, ORT_LIB, b"again");
        });
        let missing = tar_gz(|b| add_file(b, ORT_LIB, b"new-ort"));
        for (bytes, want) in [
            (symlink, "not a regular file"),
            (extra, "unexpected archive entry"),
            (duplicate, "unexpected archive entry"),
            (missing, "missing required files"),
        ] {
            let (dir, asset) = setup(sha_hex(&bytes));
            let err = install(dir.path(), &asset, serve(bytes)).unwrap_err();
            assert!(err.contains(want), "{want}: {err}");
            assert_untouched(dir.path());
        }
    }

    #[test]
    fn version_gate() {
        assert_eq!(parse_version("12.7.6"), Some((12, 7)));
        assert_eq!(parse_version("13"), Some((13, 0)));
        assert_eq!(parse_version(""), None);
        assert!(parse_version("12.7.6").unwrap() < NATIVE_MIN_MACOS);
        assert!(parse_version("13.3.1").unwrap() < NATIVE_MIN_MACOS);
        assert!(parse_version("13.4").unwrap() >= NATIVE_MIN_MACOS);
        assert!(parse_version("26.6.2").unwrap() >= NATIVE_MIN_MACOS);
    }

    #[test]
    fn pinned_assets_are_https_github_with_sha256() {
        assert!(RELEASE_BASE.starts_with("https://github.com/kevin-ping/pocket-agent/releases/download/"));
        for a in &ASSETS {
            assert_eq!(a.sha256.len(), 64, "{}", a.name);
            assert!(a.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()), "{}", a.name);
        }
    }
}
