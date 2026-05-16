use std::fmt;

use crate::config::TargetSpec;

/// All resolution targets, listed as (python_version, os, arch) where os/arch
/// are raw platform identifiers passed into the type-state builder.
///
/// Python 3.8 is the only version that keeps win32 x86; all later versions drop it.
pub const SUPPORTED_RESOLUTION_TARGETS: &[(&str, &str, &str)] = &[
    // (python, os, arch)
    ("3.8", "linux", "x86_64"),
    ("3.8", "win32", "x86"),
    ("3.8", "win32", "AMD64"),
    ("3.9", "linux", "x86_64"),
    ("3.9", "win32", "AMD64"),
    ("3.10", "linux", "x86_64"),
    ("3.10", "win32", "AMD64"),
    ("3.11", "linux", "x86_64"),
    ("3.11", "win32", "AMD64"),
    ("3.12", "linux", "x86_64"),
    ("3.12", "win32", "AMD64"),
];

pub const CPYTHON_IMPLEMENTATION_NAME: &str = "cpython";
pub const CPYTHON_IMPLEMENTATION_LABEL: &str = "CPython";
pub const DEFAULT_IMPLEMENTATION_VERSION_SUFFIX: &str = ".0";

/// Target environment: (Python version, platform).
///
/// Constructed exclusively through the type-state builder
/// (`TargetEnv::builder()` → `PlatformBuilder` → `VersionBuilder` → `BuildReady`),
/// which guarantees that all correlated fields are internally consistent.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TargetEnv {
    python_version: String,                 // "3.12"
    python_full_version: String,            // "3.12.0"
    sys_platform: String,                   // "linux" / "win32"
    platform_machine: String,               // "x86_64" / "AMD64" / "x86"
    platform_system: String,                // "Linux" / "Windows"
    os_name: String,                        // "posix" / "nt"
    implementation_name: String,            // "cpython"
    platform_python_implementation: String, // "CPython"
    implementation_version: String,         // "3.12.0"
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

impl TargetEnv {
    // ── getters ──
    pub fn python_version(&self) -> &str {
        &self.python_version
    }
    pub fn python_full_version(&self) -> &str {
        &self.python_full_version
    }
    pub fn sys_platform(&self) -> &str {
        &self.sys_platform
    }
    pub fn platform_machine(&self) -> &str {
        &self.platform_machine
    }
    pub fn platform_system(&self) -> &str {
        &self.platform_system
    }
    pub fn os_name(&self) -> &str {
        &self.os_name
    }
    pub fn implementation_name(&self) -> &str {
        &self.implementation_name
    }
    pub fn platform_python_implementation(&self) -> &str {
        &self.platform_python_implementation
    }
    pub fn implementation_version(&self) -> &str {
        &self.implementation_version
    }

    // ── entry point ──
    pub fn builder() -> PlatformBuilder {
        PlatformBuilder
    }

    /// Generate all resolution targets from constants.
    pub fn all_resolution_targets() -> Vec<TargetEnv> {
        SUPPORTED_RESOLUTION_TARGETS
            .iter()
            .filter_map(|(pv, os, arch)| {
                Some(
                    TargetEnv::builder()
                        .platform(os, arch)?
                        .python_version(pv)?
                        .build(),
                )
            })
            .collect()
    }

    /// Convert user-friendly TargetSpec into TargetEnv.
    /// Returns None if the os/arch combination is not supported.
    fn from_spec(spec: &TargetSpec) -> Option<TargetEnv> {
        Some(
            TargetEnv::builder()
                .platform(&spec.os, &spec.arch)?
                .python_version(&spec.python)?
                .build(),
        )
    }

    /// Build resolution targets from user config.
    pub fn from_specs(specs: &[TargetSpec]) -> Vec<TargetEnv> {
        specs.iter().filter_map(TargetEnv::from_spec).collect()
    }

    /// Convert to pep508_rs::MarkerEnvironment for marker evaluation.
    pub fn to_marker_env(
        &self,
    ) -> Result<pep508_rs::MarkerEnvironment, pep440_rs::VersionParseError>
    {
        use pep508_rs::MarkerEnvironmentBuilder;

        MarkerEnvironmentBuilder {
            implementation_name: self.implementation_name.as_str(),
            implementation_version: self.implementation_version.as_str(),
            os_name: self.os_name.as_str(),
            platform_machine: self.platform_machine.as_str(),
            platform_python_implementation: self
                .platform_python_implementation
                .as_str(),
            platform_release: "",
            platform_system: self.platform_system.as_str(),
            platform_version: "",
            python_full_version: self.python_full_version.as_str(),
            python_version: self.python_version.as_str(),
            sys_platform: self.sys_platform.as_str(),
        }
        .try_into()
    }

    /// Test helper — builds a TargetEnv from raw identifiers, panics on invalid input.
    pub fn test_env(os: &str, arch: &str, py: &str) -> TargetEnv {
        TargetEnv::builder()
            .platform(os, arch)
            .expect("test_env: unsupported os/arch combination")
            .python_version(py)
            .expect("test_env: invalid python version")
            .build()
    }
}

// ── type-state builder ──────────────────────────────────────────────────────

/// Phase 1: empty builder — only `.platform()` is available.
pub struct PlatformBuilder;

impl PlatformBuilder {
    /// Accept `(os, arch)`, auto-derive `sys_platform`, `platform_system`,
    /// `platform_machine`, and `os_name`.
    /// Returns `None` when the os/arch combination is unsupported.
    pub fn platform(self, os: &str, arch: &str) -> Option<VersionBuilder> {
        let (sys_platform, platform_system, platform_machine, os_name) =
            Self::map_platform(&os.to_lowercase(), &arch.to_lowercase())?;
        Some(VersionBuilder {
            sys_platform: sys_platform.to_string(),
            platform_system: platform_system.to_string(),
            platform_machine: platform_machine.to_string(),
            os_name: os_name.to_string(),
        })
    }

    fn map_platform(
        os_lower: &str,
        arch_lower: &str,
    ) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
        match (os_lower, arch_lower) {
            ("linux", "x64" | "x86_64" | "amd64") => {
                Some(("linux", "Linux", "x86_64", "posix"))
            }
            ("win32" | "windows", "x86") => {
                Some(("win32", "Windows", "x86", "nt"))
            }
            ("win32" | "windows", "x64" | "x86_64" | "amd64") => {
                Some(("win32", "Windows", "AMD64", "nt"))
            }
            _ => None,
        }
    }
}

/// Phase 2: platform fields set — only `.python_version()` is available.
pub struct VersionBuilder {
    sys_platform: String,
    platform_system: String,
    platform_machine: String,
    os_name: String,
}

impl VersionBuilder {
    /// Accept a Python version string (e.g. `"3.12"` or `"3.12.0"`), auto-derive
    /// `python_full_version` and `implementation_version`.
    /// Returns `None` when the version cannot be parsed.
    pub fn python_version(self, py: &str) -> Option<BuildReady> {
        let (python_version, python_full_version) =
            Self::parse_python_version(py)?;
        Some(BuildReady {
            sys_platform: self.sys_platform,
            platform_system: self.platform_system,
            platform_machine: self.platform_machine,
            os_name: self.os_name,
            python_version,
            python_full_version: python_full_version.clone(),
            implementation_version: python_full_version,
        })
    }

    fn parse_python_version(py: &str) -> Option<(String, String)> {
        let dot_count = py.matches('.').count();
        let pv = match dot_count {
            1 => py.to_string(),
            2 => {
                let parts: Vec<&str> = py.split('.').collect();
                format!("{}.{}", parts[0], parts[1])
            }
            _ => return None,
        };
        let full = if dot_count == 1 {
            format!("{pv}{DEFAULT_IMPLEMENTATION_VERSION_SUFFIX}")
        } else {
            py.to_string()
        };
        Some((pv, full))
    }
}

/// Phase 3: all derived fields filled — ready for `.build()`.
pub struct BuildReady {
    sys_platform: String,
    platform_system: String,
    platform_machine: String,
    os_name: String,
    python_version: String,
    python_full_version: String,
    implementation_version: String,
}

impl BuildReady {
    /// Produce the final `TargetEnv`, filling constant fields
    /// (`implementation_name`, `platform_python_implementation`).
    pub fn build(self) -> TargetEnv {
        TargetEnv {
            python_version: self.python_version,
            python_full_version: self.python_full_version,
            sys_platform: self.sys_platform,
            platform_machine: self.platform_machine,
            platform_system: self.platform_system,
            os_name: self.os_name,
            implementation_name: CPYTHON_IMPLEMENTATION_NAME.to_string(),
            platform_python_implementation: CPYTHON_IMPLEMENTATION_LABEL
                .to_string(),
            implementation_version: self.implementation_version,
        }
    }
}
