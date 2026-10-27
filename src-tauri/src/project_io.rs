//! Project files (`.mmproj`): versioned JSON with media paths stored relative
//! to the project file where possible.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::Project;

pub const EXTENSION: &str = "mmproj";
const FORMAT: &str = "music-maker-project";
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    format: String,
    version: u32,
    project: Project,
}

fn relative_to(path: &Path, base: &Path) -> Option<String> {
    path.strip_prefix(base).ok().map(|p| p.to_string_lossy().replace('\\', "/"))
}

/// Saves the project. Recordings made in the app's scratch folder are copied
/// next to the project first; returns the project with updated asset paths.
pub fn save(path: &Path, project: &Project, recordings_dir: &Path) -> Result<Project, String> {
    let dir = path.parent().ok_or("Invalid project path")?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into());
    let mut live = project.clone();
    for a in live.assets.iter_mut() {
        let p = PathBuf::from(&a.path);
        if p.starts_with(recordings_dir) {
            let audio_dir = dir.join(format!("{stem} Audio"));
            std::fs::create_dir_all(&audio_dir).map_err(|e| format!("Can't create {}: {e}", audio_dir.display()))?;
            let dest = audio_dir.join(p.file_name().unwrap_or_default());
            std::fs::copy(&p, &dest).map_err(|e| format!("Can't copy recording {}: {e}", p.display()))?;
            a.path = dest.to_string_lossy().into_owned();
        }
    }
    let mut stored = live.clone();
    for a in stored.assets.iter_mut() {
        if let Some(rel) = relative_to(Path::new(&a.path), dir) {
            a.path = rel;
        }
    }
    let file = ProjectFile { format: FORMAT.into(), version: VERSION, project: stored };
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("mmproj.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("Can't write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("Can't write {}: {e}", path.display()))?;
    Ok(live)
}

pub fn load(path: &Path) -> Result<Project, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("Can't read {}: {e}", path.display()))?;
    let file: ProjectFile = serde_json::from_str(&text).map_err(|e| format!("Not a valid project file: {e}"))?;
    if file.format != FORMAT {
        return Err("Not a Music Maker project file".into());
    }
    if file.version > VERSION {
        return Err("This project was saved by a newer version of Music Maker".into());
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut p = file.project;
    for a in p.assets.iter_mut() {
        let ap = PathBuf::from(&a.path);
        if ap.is_relative() {
            a.path = dir.join(ap).to_string_lossy().into_owned();
        }
    }
    let max_id = max_id(&p);
    if p.next_id <= max_id {
        p.next_id = max_id + 1;
    }
    Ok(p)
}

fn max_id(p: &Project) -> u64 {
    let mut m = 0;
    for t in &p.tracks {
        m = m.max(t.id);
        for c in &t.clips {
            m = m.max(c.id);
        }
        for e in &t.effects {
            m = m.max(e.id);
        }
    }
    for pat in &p.patterns {
        m = m.max(pat.id);
        for n in &pat.notes {
            m = m.max(n.id);
        }
    }
    for a in &p.assets {
        m = m.max(a.id);
    }
    for e in &p.master.effects {
        m = m.max(e.id);
    }
    m
}
