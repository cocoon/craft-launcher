//! The launcher's record of what it installed itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use artcraft_launcher_core::PackageKind;
use serde::{Deserialize, Serialize};

/// One app the launcher installed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub version: String,
    pub kind: PackageKind,
    /// The `.app`, the exe, the AppImage, or the folder of an unpacked tarball.
    pub path: PathBuf,
    /// Unix seconds.
    pub installed_at: i64,
    /// Every file installed, for tree installs (FreeBSD), so removal takes exactly those.
    #[serde(default)]
    pub files: Vec<PathBuf>,
    /// The files are in a system prefix and removing them needs root.
    #[serde(default)]
    pub needs_root: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub apps: BTreeMap<String, Record>,
}

fn launcher_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

/// Normalize Windows extended-length paths (`\\?\...`) before comparing
/// them with ordinary drive paths. Keep UNC paths as ordinary UNC paths.
#[cfg(windows)]
fn normalize_windows_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix("\\\\?\\UNC\\") {
        PathBuf::from(format!("\\\\{}", rest))
    } else if let Some(rest) = text.strip_prefix("\\\\?\\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

#[cfg(windows)]
fn same_component(left: &std::path::Component<'_>, right: &std::path::Component<'_>) -> bool {
    use std::path::Component;
    match (left, right) {
        (Component::Prefix(a), Component::Prefix(b)) => {
            a.as_os_str().to_string_lossy().eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
        }
        (Component::RootDir, Component::RootDir) => true,
        (Component::Normal(a), Component::Normal(b)) => {
            a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
        }
        (Component::CurDir, Component::CurDir) => true,
        (Component::ParentDir, Component::ParentDir) => true,
        _ => false,
    }
}

#[cfg(not(windows))]
fn same_component(left: &std::path::Component<'_>, right: &std::path::Component<'_>) -> bool {
    left == right
}

/// Build a lexical path from `base` to `target`. Both paths must be absolute
/// and on the same volume (on Windows).
fn relative_path(base: &Path, target: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    let (base_storage, target_storage) =
        (normalize_windows_path(base), normalize_windows_path(target));
    #[cfg(not(windows))]
    let (base_storage, target_storage) = (base.to_path_buf(), target.to_path_buf());
    let base = base_storage.as_path();
    let target = target_storage.as_path();

    if !base.is_absolute() || !target.is_absolute() {
        return None;
    }

    let base_components: Vec<_> = base.components().collect();
    let target_components: Vec<_> = target.components().collect();
    let common = base_components
        .iter()
        .zip(&target_components)
        .take_while(|(left, right)| same_component(left, right))
        .count();

    // A different Windows drive/prefix cannot be expressed as a relative path.
    if common == 0 {
        return None;
    }

    let mut relative = PathBuf::new();
    for component in &base_components[common..] {
        if matches!(component, std::path::Component::Normal(_)) {
            relative.push("..");
        } else {
            return None;
        }
    }
    for component in &target_components[common..] {
        relative.push(component.as_os_str());
    }
    Some(if relative.as_os_str().is_empty() { PathBuf::from(".") } else { relative })
}

impl Manifest {
    pub fn load(path: &Path) -> Self {
        let mut manifest: Self = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        if let Some(root) = launcher_dir() {
            for record in manifest.apps.values_mut() {
                if record.path.is_relative() {
                    record.path = root.join(&record.path);
                }
                for file in &mut record.files {
                    if file.is_relative() {
                        *file = root.join(&*file);
                    }
                }
            }
        }
        manifest
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let mut stored = self.clone();
        let root = launcher_dir().ok_or("couldn't determine the launcher's folder to store relative installation paths")?;
        for record in stored.apps.values_mut() {
            record.path = relative_path(&root, &record.path).ok_or_else(|| {
                format!("installation path {} cannot be stored relative to the launcher folder", record.path.display())
            })?;
            for file in &mut record.files {
                *file = relative_path(&root, file).ok_or_else(|| {
                    format!("installed file {} cannot be stored relative to the launcher folder", file.display())
                })?;
            }
        }
        let bytes = serde_json::to_vec_pretty(&stored).map_err(|e| e.to_string())?;
        crate::paths::write_atomic(path, &bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_relative_paths_including_parent_segments() {
        let base = Path::new("/opt/ArtCraft");
        assert_eq!(relative_path(base, base), Some(PathBuf::from(".")));
        assert_eq!(relative_path(base, Path::new("/opt/ArtCraft/Apps/Test")), Some(PathBuf::from("Apps/Test")));
        assert_eq!(relative_path(base, Path::new("/opt/Other/App")), Some(PathBuf::from("../Other/App")));
    }

    #[test]
    fn round_trips_and_survives_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("installed.json");
        assert_eq!(Manifest::load(&path), Manifest::default());
        let mut m = Manifest::default();
        m.apps.insert(
            "photocraft".into(),
            Record { version: "0.5.0".into(), kind: PackageKind::AppImage, path: "/x/y".into(), installed_at: 1, files: vec![], needs_root: false },
        );
        m.save(&path).unwrap();
        assert_eq!(Manifest::load(&path), m);
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(Manifest::load(&path), Manifest::default());
    }
}
