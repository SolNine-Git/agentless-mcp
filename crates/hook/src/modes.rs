//! Each gated check's mode: the repository entry, then the environment default, then advisory.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

use io::{ConfigWarning, GateMode, GatedCheck, ProjectConfig};
use serde::{Serialize, Serializer};

use crate::ENV_GATE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Modes([GateMode; 3]);

impl Modes {
    pub(crate) fn of(self, check: GatedCheck) -> GateMode {
        self.0[check as usize]
    }

    pub(crate) fn active(self) -> Vec<GatedCheck> {
        GatedCheck::ALL
            .into_iter()
            .filter(|check| self.of(*check) != GateMode::Off)
            .collect()
    }

    pub(crate) fn iter(self) -> impl Iterator<Item = (GatedCheck, GateMode)> {
        GatedCheck::ALL.into_iter().zip(self.0)
    }
}

impl Serialize for Modes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.iter().map(|(check, mode)| (check.name(), mode.name())))
    }
}

// The modes, and the warning for an environment value that names no default mode.
pub(crate) fn resolve(
    entries: Option<&BTreeMap<GatedCheck, GateMode>>,
    env_value: Option<&OsStr>,
) -> (Modes, Option<String>) {
    let (default, warning) = default_mode(env_value);
    let mode = |check: GatedCheck| {
        entries
            .and_then(|entries| entries.get(&check).copied())
            .unwrap_or(default)
    };
    (Modes(GatedCheck::ALL.map(mode)), warning)
}

fn default_mode(env_value: Option<&OsStr>) -> (GateMode, Option<String>) {
    let Some(value) = env_value.filter(|value| !value.is_empty()) else {
        return (GateMode::Advisory, None);
    };
    let named = [GateMode::Advisory, GateMode::Block]
        .into_iter()
        .find(|mode| value == mode.name());
    if let Some(mode) = named {
        return (mode, None);
    }
    let shown = text::CodePointString::from_utf8_escaped(value.as_bytes()).quoted();
    let warning = format!("{ENV_GATE}={shown} is not off, advisory, or block, so advisory applies");
    (GateMode::Advisory, Some(warning))
}

// The warnings about the file itself and about the `erosion_gate` key, in the config's order.
pub(crate) fn config_warnings(config: &ProjectConfig) -> Vec<String> {
    config
        .warnings
        .iter()
        .filter(|warning| {
            matches!(
                warning,
                ConfigWarning::File(_) | ConfigWarning::ErosionGate(_)
            )
        })
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
mod tests;
