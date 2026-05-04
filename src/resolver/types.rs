use std::fmt;

/// Target environment: (Python version, platform).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TargetEnv {
    pub python_version: String,      // "3.12"
    pub python_full_version: String, // "3.12.0"
    pub sys_platform: String,        // "linux" / "win32"
    pub platform_machine: String,    // "x86_64" / "AMD64" / "x86"
}

impl fmt::Display for TargetEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "py{}/{}/{}",
            self.python_version, self.sys_platform, self.platform_machine
        )
    }
}

/// All 21 targets: 7 Python versions × 3 platforms.
pub fn all_targets() -> [TargetEnv; 21] {
    let py_versions = ["3.8", "3.9", "3.10", "3.11", "3.12", "3.13", "3.14"];
    let platforms = [("linux", "x86_64"), ("win32", "x86"), ("win32", "AMD64")];

    std::array::from_fn(|i| {
        let pv_idx = i / 3;
        let plat_idx = i % 3;
        let pv = py_versions[pv_idx];
        TargetEnv {
            python_version: pv.to_string(),
            python_full_version: format!("{pv}.0"),
            sys_platform: platforms[plat_idx].0.to_string(),
            platform_machine: platforms[plat_idx].1.to_string(),
        }
    })
}
