//! The session markers: one file per unlocked session, named by the SHA-256 of the session id, in
//! a fixed directory that both hooks share and that a reboot clears.

use std::path::PathBuf;

use json::JsonValue;
use text::CodePointString;

use super::{Gate, Suppressed, stat};
use crate::text::sha256_hex;

pub(crate) const MARKER_DIR: &str = "/tmp/agentless_gate";
const PROBE_PREFIX: &str = ".write-probe-";
const MARKER_VERSION: &str = "2";

// A session id with a lone surrogate has no UTF-8 bytes: the reference's encode raises.
fn path(gate: &Gate<'_>, session: &CodePointString) -> Result<PathBuf, Suppressed> {
    let session = session.as_str().ok_or(Suppressed)?;
    let name = format!("{}.json", sha256_hex(session.as_bytes()));
    Ok(gate.markers.join(name))
}

// Another local user can own or write the shared directory and plant symlinks in it, so markers
// are read and written only in a real directory that this user owns and nobody else can write.
fn trusted(gate: &Gate<'_>) -> bool {
    matches!(io::is_private_directory(&gate.markers), Ok(true))
}

pub(crate) fn exists(gate: &Gate<'_>, session: &CodePointString) -> Result<bool, Suppressed> {
    let path = path(gate, session)?;
    if !trusted(gate) {
        return Ok(false);
    }
    stat::exists(&path)
}

// Proves that a marker can be written before the gate enforces its absence. The probe is a new
// file: an entry already at its name, a symlink included, fails it.
pub(crate) fn storage_ready(gate: &Gate<'_>) -> bool {
    let probe = gate.markers.join(format!("{PROBE_PREFIX}{}", gate.pid));
    io::create_missing_owner_dir(&gate.markers).is_ok()
        && trusted(gate)
        && io::write_new_file(&probe, b"").is_ok()
        && matches!(io::remove_file(&probe), Ok(true))
}

// The directory comes first: the reference creates it before it encodes the session id. The
// rename of a private temporary file never writes through a symlink at the marker's name.
pub(crate) fn write(
    gate: &Gate<'_>,
    session: &CodePointString,
    reason: &str,
    tool: &JsonValue,
) -> Result<(), Suppressed> {
    io::create_missing_owner_dir(&gate.markers).map_err(|_| Suppressed)?;
    let path = path(gate, session)?;
    if !trusted(gate) {
        return Err(Suppressed);
    }
    let receipt = JsonValue::Object(vec![
        ("version".into(), JsonValue::Int(MARKER_VERSION.to_owned())),
        ("reason".into(), JsonValue::Str(reason.into())),
        ("tool".into(), tool.clone()),
    ]);
    io::replace_file(&path, json::value_text(&receipt).as_bytes()).map_err(|_| Suppressed)
}
