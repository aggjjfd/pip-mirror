use std::fmt;

use type_state_builder::TypeStateBuilder;

use crate::config::TargetSpec;

/// All resolution targets, listed explicitly as (python_version, sys_platform,
/// platform_system, platform_machine, os_name).
///
/// Python 3.8 is the only version that keeps win32 x86; all later versions drop it.
pub const SUPPORTED_RESOLUTION_TARGETS: &[(&str, &str, &str, &str, &str)] = &[
    // Python 3.8
    ("3.8", "linux", "Linux", "x86_64", "posix"),
    ("3.8", "win32", "Windows", "x86", "nt"),
    ("3.8", "win32", "Windows", "AMD64", "nt"),
    // Python 3.9
    ("3.9", "linux", "Linux", "x86_64", "posix"),
    ("3.9", "win32", "Windows", "AMD64", "nt"),
    // Python 3.10
    ("3.10", "linux", "Linux", "x86_64", "posix"),
    ("3.10", "win32", "Windows", "AMD64", "nt"),
    // Python 3.11
    ("3.11", "linux", "Linux", "x86_64", "posix"),
    ("3.11", "win32", "Windows", "AMD64", "nt"),
    // Python 3.12
    ("3.12", "linux", "Linux", "x86_64", "posix"),
    ("3.12", "win32", "Windows", "AMD64", "nt"),
];

pub const CPYTHON_IMPLEMENTATION_NAME: &str = "cpython";
pub const CPYTHON_IMPLEMENTATION_LABEL: &str = "CPython";
pub const DEFAULT_IMPLEMENTATION_VERSION_SUFFIX: &str = ".0";

/// Target environment: (Python version, platform).
#[derive(Debug, Clone, PartialEq, Eq, Hash, TypeStateBuilder)]
#[builder(impl_into)]
pub struct TargetEnv {
    #[builder(required)]
    pub python_version: String, // "3.12"
    #[builder(required)]
    pub python_full_version: String, // "3.12.0"
    #[builder(required)]
    pub sys_platform: String, // "linux" / "win32"
    #[builder(required)]
    pub platform_machine: String, // "x86_64" / "AMD64" / "x86"
    #[builder(required)]
    pub platform_system: String, // "Linux" / "Windows"
    #[builder(required)]
    pub os_name: String, // "posix" / "nt"
    #[builder(required)]
    pub implementation_name: String, // "cpython"
    #[builder(required)]
    pub platform_python_implementation: String, // "CPython"
    #[builder(required)]
    pub implementation_version: String, // "3.12.0"
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
        SUPPORTED_RESOLUTION_TARGETS
            .iter()
            .map(|(pv, sys, sys_name, machine, os)| {
                let full =
                    format!("{pv}{DEFAULT_IMPLEMENTATION_VERSION_SUFFIX}");
                TargetEnv::builder()
                    .python_version(pv.to_string())
                    .python_full_version(full.clone())
                    .sys_platform(sys.to_string())
                    .platform_machine(machine.to_string())
                    .platform_system(sys_name.to_string())
                    .os_name(os.to_string())
                    .implementation_name(
                        CPYTHON_IMPLEMENTATION_NAME.to_string(),
                    )
                    .platform_python_implementation(
                        CPYTHON_IMPLEMENTATION_LABEL.to_string(),
                    )
                    .implementation_version(full.clone())
                    .build()
            })
            .collect()
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

    fn parse_python_version(spec: &TargetSpec) -> Option<(String, String)> {
        let dot_count = spec.python.matches('.').count();
        let pv = match dot_count {
            1 => spec.python.clone(),
            2 => {
                let parts: Vec<&str> = spec.python.split('.').collect();
                format!("{}.{}", parts[0], parts[1])
            }
            _ => return None,
        };
        let full = if dot_count == 1 {
            format!("{pv}{DEFAULT_IMPLEMENTATION_VERSION_SUFFIX}")
        } else {
            spec.python.clone()
        };
        Some((pv, full))
    }

    /// Convert user-friendly TargetSpec into TargetEnv.
    /// Returns None if the os/arch combination is not supported.
    fn from_spec(spec: &TargetSpec) -> Option<TargetEnv> {
        let (sys_platform, platform_system, platform_machine, os_name) =
            Self::map_platform(
                &spec.os.to_lowercase(),
                &spec.arch.to_lowercase(),
            )?;
        let (python_version, python_full_version) =
            Self::parse_python_version(spec)?;
        Some(
            TargetEnv::builder()
                .python_version(python_version)
                .python_full_version(python_full_version.clone())
                .sys_platform(sys_platform.to_string())
                .platform_machine(platform_machine.to_string())
                .platform_system(platform_system.to_string())
                .os_name(os_name.to_string())
                .implementation_name(CPYTHON_IMPLEMENTATION_NAME.to_string())
                .platform_python_implementation(
                    CPYTHON_IMPLEMENTATION_LABEL.to_string(),
                )
                .implementation_version(python_full_version)
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
}
