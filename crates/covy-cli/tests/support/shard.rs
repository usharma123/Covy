use assert_cmd::Command;

pub fn covy_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("covy")
}
