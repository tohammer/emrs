//! Runs the built `cargo-emacs` binary against fixture crates: `build`.

mod common;

use common::*;

#[test]
fn default_destination() {
    let fixture = Fixture::cdylib("default");
    let output = fixture.run(&["build"]);
    let printed = printed_path(&output);
    assert!(printed.is_absolute(), "{}", printed.display());
    assert!(printed.exists());
    let expected = fixture.path(&format!("target/emacs/fixture-mod-module.{EXT}"));
    assert!(same_file(&printed, &expected));
}

#[test]
fn dir_option() {
    let fixture = Fixture::cdylib("dir");
    let output = fixture.run(&["build", "--dir", "out"]);
    let printed = printed_path(&output);
    assert!(same_file(&printed, &fixture.path(&format!("out/fixture-mod-module.{EXT}"))));
    assert!(!fixture.path("target/emacs").exists());
}

#[test]
fn metadata_name_override() {
    let fixture =
        Fixture::new("override", "[package.metadata.emacs-module]\nname = \"other\"\n", "cdylib", GOOD_SOURCE);
    let output = fixture.run(&["build"]);
    let printed = printed_path(&output);
    assert!(same_file(&printed, &fixture.path(&format!("target/emacs/other-module.{EXT}"))));
    assert!(!fixture.path(&format!("target/emacs/fixture-mod-module.{EXT}")).exists());
}

#[test]
fn invalid_name_installs_nothing() {
    let fixture = Fixture::new("badname", "[package.metadata.emacs-module]\nname = \"a/b\"\n", "cdylib", GOOD_SOURCE);
    let output = fixture.run(&["build"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("cargo emacs: "), "{}", stderr(&output));
    assert!(!fixture.path("target/emacs").exists());
}

#[test]
fn no_cdylib_is_an_error() {
    let fixture = Fixture::new("nocdylib", "", "rlib", GOOD_SOURCE);
    let output = fixture.run(&["build"]);
    assert_eq!(output.status.code(), Some(1));
    let err = stderr(&output);
    assert!(err.contains("cargo emacs: ") && err.contains("cdylib"), "{err}");
    assert!(stdout(&output).is_empty());
    assert!(!fixture.path("target/emacs").exists());
}

#[test]
fn build_error_returns_cargos_status() {
    let fixture = Fixture::new("builderror", "", "cdylib", "this is not rust\n");
    let output = fixture.run(&["build"]);
    assert_eq!(output.status.code(), Some(101), "{}", stderr(&output));
    assert!(stdout(&output).is_empty());
    assert!(!fixture.path("target/emacs").exists());
}

#[test]
fn second_build_replaces_the_file() {
    let fixture = Fixture::cdylib("replace");
    let first = printed_path(&fixture.run(&["build"]));
    let before = std::fs::read(&first).unwrap();
    fixture.set_source("#[no_mangle]\npub extern \"C\" fn fixture_other() -> i32 {\n    7\n}\n");
    let second = printed_path(&fixture.run(&["build"]));
    assert!(same_file(&first, &second));
    let after = std::fs::read(&second).unwrap();
    assert_ne!(before, after);
    let built = fixture.path(&format!("target/debug/{}fixture_mod.{EXT}", if cfg!(windows) { "" } else { "lib" }));
    assert_eq!(after, std::fs::read(built).unwrap());
}

#[test]
fn message_format_is_a_usage_error() {
    let fixture = Fixture::cdylib("msgformat");
    let output = fixture.run(&["build", "--message-format=json"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).starts_with("cargo emacs: "));
}

#[test]
fn version_and_help() {
    let fixture = Fixture::cdylib("version");
    let output = fixture.run(&["--version"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output), format!("cargo-emacs {}\n", env!("CARGO_PKG_VERSION")));
    let output = fixture.run(&["--help"]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("cargo emacs build"));
}
