use std::path::Path;

use super::PythonBuildEntry;

pub fn build_python_builds_index(
    entries: &[PythonBuildEntry],
    repo: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut meta = serde_json::Map::new();
    for entry in entries {
        let mut e = entry.raw.clone();
        e["url"] = serde_json::Value::String(format!(
            "/python-builds/{}",
            entry.filename
        ));
        // uv treats Some("") prerelease as a prerelease → skip stable builds.
        if e.get("prerelease").and_then(|v| v.as_str()) == Some("") {
            e["prerelease"] = serde_json::Value::Null;
        }
        meta.insert(entry.key.clone(), e);
    }
    std::fs::write(
        repo.join("python-builds/index.json"),
        serde_json::to_string_pretty(&meta)?,
    )?;
    Ok(())
}
