use std::fmt;

pub const SUPPORTED_PYTHON_MINORS: &[&str] =
    &["3.8", "3.9", "3.10", "3.11", "3.12"];

pub const SUPPORTED_RESOLUTION_TARGETS: &[(&str, &str, &str, &str)] = &[
    ("linux", "Linux", "x86_64", "posix"),
    ("win32", "Windows", "x86", "nt"),
    ("win32", "Windows", "AMD64", "nt"),
];

pub const CPYTHON_IMPLEMENTATION_NAME: &str = "cpython";
pub const CPYTHON_IMPLEMENTATION_LABEL: &str = "CPython";
pub const DEFAULT_IMPLEMENTATION_VERSION_SUFFIX: &str = ".0";

/// Target environment: (Python version, platform).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TargetEnv {
    pub python_version: String,                 // "3.12"
    pub python_full_version: String,            // "3.12.0"
    pub sys_platform: String,                   // "linux" / "win32"
    pub platform_machine: String,               // "x86_64" / "AMD64" / "x86"
    pub platform_system: String,                // "Linux" / "Windows"
    pub os_name: String,                        // "posix" / "nt"
    pub implementation_name: String,            // "cpython"
    pub platform_python_implementation: String, // "CPython"
    pub implementation_version: String,         // "3.12.0"
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
    /// Generate all resolution targets from constants.
    pub fn all_resolution_targets() -> Vec<TargetEnv> {
        SUPPORTED_PYTHON_MINORS
            .iter()
            .flat_map(|pv| Self::build_targets_for_python_version(pv))
            .collect()
    }

    /// Build all target environments for a single Python version.
    fn build_targets_for_python_version(pv: &str) -> Vec<TargetEnv> {
        let full = format!("{pv}{DEFAULT_IMPLEMENTATION_VERSION_SUFFIX}");
        SUPPORTED_RESOLUTION_TARGETS
            .iter()
            .map(|(sys, sys_name, machine, os)| TargetEnv {
                python_version: pv.to_string(),
                python_full_version: full.clone(),
                sys_platform: sys.to_string(),
                platform_machine: machine.to_string(),
                platform_system: sys_name.to_string(),
                os_name: os.to_string(),
                implementation_name: CPYTHON_IMPLEMENTATION_NAME.to_string(),
                platform_python_implementation: CPYTHON_IMPLEMENTATION_LABEL
                    .to_string(),
                implementation_version: full.clone(),
            })
            .collect()
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
}
