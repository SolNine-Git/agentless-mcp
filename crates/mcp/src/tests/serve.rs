use std::ffi::OsString;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use app::EnvConfig;

use crate::serve::{indexed, opted_out, watched};
use crate::{ServerConfig, Start, Transport};

fn env_with(name: &str, value: OsString) -> EnvConfig {
    EnvConfig::new([(OsString::from(name), value)], PathBuf::from("/"))
}

fn start(transport: Transport, no_auto_index: bool, no_auto_restart: bool) -> Start {
    let config = ServerConfig {
        roots: Vec::new(),
        roots_files: Vec::new(),
        launch_dir: None,
        max_concurrency: 8,
        call_limit: 30.0,
        recache_grace: 60.0,
    };
    Start {
        config,
        transport,
        no_auto_index,
        no_auto_restart,
        relaunch: None,
    }
}

fn http() -> Transport {
    Transport::Http(SocketAddr::from((Ipv4Addr::LOCALHOST, 8000)))
}

// Unset, empty, "0", "false" and "False" keep a feature on; any other value turns it off.
#[test]
fn an_opt_out_variable_keeps_the_feature_only_for_the_reference_values() {
    let name = "AGENTLESS_MCP_NO_AUTO_INDEX";
    let unset = EnvConfig::new(Vec::new(), PathBuf::from("/"));
    assert!(!opted_out(&unset, name));
    for kept in ["", "0", "false", "False"] {
        assert!(
            !opted_out(&env_with(name, OsString::from(kept)), name),
            "{kept:?}"
        );
    }
    for off in ["1", "true", "FALSE", " 0", "no"] {
        assert!(
            opted_out(&env_with(name, OsString::from(off)), name),
            "{off:?}"
        );
    }
    assert!(opted_out(
        &env_with(name, OsString::from_vec(vec![0xff])),
        name
    ));
}

#[test]
fn only_an_http_server_watches_its_install() {
    let unset = EnvConfig::new(Vec::new(), PathBuf::from("/"));
    assert!(!watched(&start(Transport::Stdio, false, false), &unset));
    assert!(watched(&start(http(), false, false), &unset));
    assert!(!watched(&start(http(), false, true), &unset));
    let off = env_with("AGENTLESS_MCP_NO_AUTO_RESTART", OsString::from("1"));
    assert!(!watched(&start(http(), false, false), &off));
}

#[test]
fn the_flag_or_the_variable_turns_the_background_index_off() {
    let unset = EnvConfig::new(Vec::new(), PathBuf::from("/"));
    assert!(indexed(&start(Transport::Stdio, false, false), &unset));
    assert!(!indexed(&start(Transport::Stdio, true, false), &unset));
    let off = env_with("AGENTLESS_MCP_NO_AUTO_INDEX", OsString::from("yes"));
    assert!(!indexed(&start(http(), false, false), &off));
}
