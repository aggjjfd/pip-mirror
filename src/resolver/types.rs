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

/// All targets: N Python versions × M platforms.
pub fn all_targets() -> Vec<TargetEnv> {
    let py_versions = ["3.8", "3.9", "3.10", "3.11", "3.12", "3.13", "3.14"];
    let platforms = [("linux", "x86_64"), ("win32", "x86"), ("win32", "AMD64")];

    let mut targets = Vec::new();
    for pv in py_versions {
        for (sys, machine) in platforms {
            targets.push(TargetEnv {
                python_version: pv.to_string(),
                python_full_version: format!("{pv}.0"),
                sys_platform: sys.to_string(),
                platform_machine: machine.to_string(),
            });
        }
    }
    targets
}
