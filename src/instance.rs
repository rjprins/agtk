use std::fmt;
use std::path::{Path, PathBuf};

const MAX_INSTANCE_NAME_CHARS: usize = 48;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstanceName(String);

impl InstanceName {
    pub fn parse(value: &str) -> Result<Self, InvalidInstanceName> {
        let valid = !value.is_empty()
            && value.len() <= MAX_INSTANCE_NAME_CHARS
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
        if valid {
            Ok(Self(value.to_owned()))
        } else {
            Err(InvalidInstanceName)
        }
    }

    pub fn from_environment() -> Result<Self, InvalidInstanceName> {
        let value = std::env::var("AGMUX_INSTANCE").unwrap_or_else(|_| "default".to_owned());
        Self::parse(&value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn application_id(&self) -> String {
        let suffix = self.0.replace('-', "_");
        format!("nl.rutger.AgmuxNative.Devel.i_{suffix}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidInstanceName;

impl fmt::Display for InvalidInstanceName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "instance name must contain only ASCII letters, digits, underscores, or hyphens",
        )
    }
}

impl std::error::Error for InvalidInstanceName {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstancePaths {
    name: InstanceName,
    runtime_dir: PathBuf,
    state_dir: PathBuf,
}

impl InstancePaths {
    pub fn new(name: InstanceName, runtime_root: &Path, state_root: &Path) -> Self {
        let runtime_dir = runtime_root.join("agmux-native").join(name.as_str());
        let state_dir = state_root.join("agmux-native").join(name.as_str());
        Self {
            name,
            runtime_dir,
            state_dir,
        }
    }

    pub fn name(&self) -> &InstanceName {
        &self.name
    }

    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    pub fn control_socket(&self) -> PathBuf {
        self.runtime_dir.join("control.sock")
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.runtime_dir.join("sessions")
    }

    pub fn captures_dir(&self) -> PathBuf {
        self.runtime_dir.join("captures")
    }

    pub fn database(&self) -> PathBuf {
        self.state_dir.join("agmux.db")
    }
}
