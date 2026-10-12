use std::ffi::OsStr;

use io::testing::cases;
use io::{GateMode, GatedCheck};

use super::{config_warnings, resolve};
use crate::testing::{MANIFEST_DIR, Sandbox, latin};

// One generated case: the name, the config bytes (None: no file), the environment value, the
// modes as CPython's json.dumps writes them, and the warnings the log gets.
type ModesCase = (String, Option<String>, Option<String>, String, Vec<String>);

#[test]
fn the_modes_and_the_warnings_match_the_generated_cases() {
    let all: Vec<ModesCase> = cases(MANIFEST_DIR, "modes", "modes");
    assert!(!all.is_empty());
    for (name, config, env, expected_modes, expected_warnings) in all {
        let sandbox = Sandbox::new();
        let root = sandbox.mkdir("repo");
        if let Some(config) = &config {
            sandbox.put("repo/.agentless-mcp.json", latin(config));
        }
        let loaded = io::load_config(&root);
        let (modes, env_warning) =
            resolve(loaded.erosion_gate.as_ref(), env.as_deref().map(OsStr::new));
        let shown = json::to_text(&modes, json::JsonForm::Compact).expect("modes serialize");
        assert_eq!(shown, expected_modes, "case {name}");
        let mut warnings = config_warnings(&loaded);
        warnings.extend(env_warning);
        assert_eq!(warnings, expected_warnings, "case {name}");
        assert_eq!(
            app::config_warnings(&loaded).len(),
            loaded.warnings.len(),
            "case {name}: one text per warning, in order"
        );
    }
}

#[test]
fn the_active_checks_are_the_ones_not_off_in_config_order() {
    let entries = [
        (GatedCheck::CloneDelta, GateMode::Block),
        (GatedCheck::ComplexityDelta, GateMode::Off),
    ]
    .into_iter()
    .collect();
    let (modes, _) = resolve(Some(&entries), None);
    assert_eq!(
        modes.active(),
        [GatedCheck::ContractDrift, GatedCheck::CloneDelta]
    );
}

#[test]
fn each_gated_check_is_a_pair_check_of_the_same_name_in_the_same_order() {
    let gated = GatedCheck::ALL.map(GatedCheck::name);
    let pairs = app::PairCheck::ALL.map(app::PairCheck::name);
    assert_eq!(gated, pairs);
}
