//! Runs the built `cargo-emacs` binary against fixture crates in temporary directories.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const EXT: &str = if cfg!(target_os = "windows") {
    "dll"
} else if cfg!(target_os = "macos") {
    "dylib"
} else {
    "so"
};

/// A fixture crate in a fresh temporary directory, removed on drop.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(test: &str, extra_manifest: &str, crate_type: &str, source: &str) -> Self {
        let root = std::env::temp_dir().join(format!("cargo-emacs-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        let manifest = format!(
            "[package]\nname = \"fixture-mod\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [lib]\ncrate-type = [\"{crate_type}\"]\n\n{extra_manifest}\n[workspace]\n"
        );
        std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
        let fixture = Fixture { root };
        fixture.set_source(source);
        fixture
    }

    fn cdylib(test: &str) -> Self {
        Self::new(test, "", "cdylib", GOOD_SOURCE)
    }

    fn set_source(&self, source: &str) {
        std::fs::write(self.root.join("src/lib.rs"), source).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-emacs"));
        command.current_dir(&self.root).arg("emacs").args(args);
        for (key, _) in std::env::vars_os() {
            let key_str = key.to_string_lossy();
            if key_str.starts_with("CARGO_") && key_str != "CARGO_HOME" || key_str == "RUSTFLAGS" {
                command.env_remove(&key);
            }
        }
        command.output().unwrap()
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

const GOOD_SOURCE: &str = "#[no_mangle]\npub extern \"C\" fn fixture_answer() -> i32 {\n    42\n}\n";

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The path printed on stdout, which must be the only line.
fn printed_path(output: &Output) -> PathBuf {
    assert!(output.status.success(), "{}", stderr(output));
    let text = stdout(output);
    let mut lines = text.lines();
    let path = PathBuf::from(lines.next().expect("a path on stdout"));
    assert!(lines.next().is_none(), "unexpected stdout: {text}");
    path
}

fn same_file(a: &Path, b: &Path) -> bool {
    a.canonicalize().unwrap() == b.canonicalize().unwrap()
}

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
