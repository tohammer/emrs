//! Runs the built `cargo-emacs` binary against fixture crates: `package`.

mod common;

use common::*;

/// The platform name of the machine running the tests.
const HOST_PLATFORM: &str = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    "linux-x86_64"
} else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
    "linux-aarch64"
} else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
    "macos-aarch64"
} else {
    "windows-x86_64"
};

#[test]
fn default_destination_is_a_release_build() {
    let fixture = Fixture::cdylib("pkg-default");
    let output = fixture.run(&["package"]);
    let printed = printed_path(&output);
    assert!(printed.is_absolute(), "{}", printed.display());
    let expected = fixture.path(&format!("target/emacs/dist/fixture-mod-module-{HOST_PLATFORM}.{EXT}"));
    assert!(same_file(&printed, &expected));
    let built = fixture.path(&format!("target/release/{}fixture_mod.{EXT}", if cfg!(windows) { "" } else { "lib" }));
    assert_eq!(std::fs::read(&printed).unwrap(), std::fs::read(built).unwrap());
    assert!(!fixture.path("target/debug").exists());
}

#[test]
fn dir_option() {
    let fixture = Fixture::cdylib("pkg-dir");
    let printed = printed_path(&fixture.run(&["package", "--dir", "out"]));
    assert!(same_file(&printed, &fixture.path(&format!("out/fixture-mod-module-{HOST_PLATFORM}.{EXT}"))));
    assert!(!fixture.path("target/emacs").exists());
}

#[test]
fn platform_overrides_the_name() {
    let fixture = Fixture::cdylib("pkg-platform");
    let printed = printed_path(&fixture.run(&["package", "--platform", "my-plat_1"]));
    assert!(same_file(&printed, &fixture.path(&format!("target/emacs/dist/fixture-mod-module-my-plat_1.{EXT}"))));
}

#[test]
fn profile_option_is_respected() {
    let fixture = Fixture::cdylib("pkg-profile");
    printed_path(&fixture.run(&["package", "--profile", "dev"]));
    assert!(fixture.path("target/debug").exists());
    assert!(!fixture.path("target/release").exists());
}

#[test]
fn unknown_target_fails_before_cargo_runs() {
    let fixture = Fixture::cdylib("pkg-unknown");
    for args in [
        &["package", "--target", "riscv64gc-unknown-linux-gnu"][..],
        &["package", "--target=riscv64gc-unknown-linux-gnu"][..],
    ] {
        let output = fixture.run(args);
        assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
        let err = stderr(&output);
        assert!(err.starts_with("cargo emacs: ") && err.contains("riscv64gc") && err.contains("--platform"), "{err}");
        assert!(stdout(&output).is_empty());
        assert!(!fixture.path("target").exists());
    }
}

#[test]
fn invalid_platform_is_a_usage_error() {
    let fixture = Fixture::cdylib("pkg-badplatform");
    let output = fixture.run(&["package", "--platform", "Bad/Name"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).starts_with("cargo emacs: "));
    assert!(!fixture.path("target").exists());
}

#[test]
fn help_lists_package() {
    let fixture = Fixture::cdylib("pkg-help");
    let output = fixture.run(&["--help"]);
    assert!(stdout(&output).contains("cargo emacs package"));
    let output = fixture.run(&["package", "--help"]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("--platform") && text.contains("--zigbuild") && text.contains("--dir"), "{text}");
}
