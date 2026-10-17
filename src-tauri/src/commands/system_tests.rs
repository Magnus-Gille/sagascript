use super::*;

#[test]
fn the_three_supported_systems_keep_their_wire_names() {
    assert_eq!(platform_name("macos"), "macos");
    assert_eq!(platform_name("windows"), "windows");
    assert_eq!(platform_name("linux"), "linux");
}

#[test]
fn any_other_system_is_unknown() {
    for os in [
        "freebsd", "android", "ios", "solaris", "", "Linux", "darwin",
    ] {
        assert_eq!(platform_name(os), "unknown", "{os:?}");
    }
}

/// What the `cfg` blocks answered before `std::env::consts::OS` replaced
/// them, computed the old way, must be what this host answers now.
#[test]
fn this_host_answers_what_the_cfg_blocks_answered() {
    let old = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    };
    assert_eq!(platform_name(std::env::consts::OS), old);
}
