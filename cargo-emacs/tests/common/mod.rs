//! Fixture crates in temporary directories and helpers to run the built `cargo-emacs` binary on them.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const EXT: &str = if cfg!(target_os = "windows") {
    "dll"
} else if cfg!(target_os = "macos") {
    "dylib"
} else {
    "so"
};

/// A fixture crate in a fresh temporary directory, removed on drop.
pub struct Fixture {
    pub root: PathBuf,
}

impl Fixture {
    pub fn new(test: &str, extra_manifest: &str, crate_type: &str, source: &str) -> Self {
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

    pub fn cdylib(test: &str) -> Self {
        Self::new(test, "", "cdylib", GOOD_SOURCE)
    }

    pub fn set_source(&self, source: &str) {
        std::fs::write(self.root.join("src/lib.rs"), source).unwrap();
    }

    pub fn run(&self, args: &[&str]) -> Output {
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

    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub const GOOD_SOURCE: &str = "#[no_mangle]\npub extern \"C\" fn fixture_answer() -> i32 {\n    42\n}\n";

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The path printed on stdout, which must be the only line.
pub fn printed_path(output: &Output) -> PathBuf {
    assert!(output.status.success(), "{}", stderr(output));
    let text = stdout(output);
    let mut lines = text.lines();
    let path = PathBuf::from(lines.next().expect("a path on stdout"));
    assert!(lines.next().is_none(), "unexpected stdout: {text}");
    path
}

pub fn same_file(a: &Path, b: &Path) -> bool {
    a.canonicalize().unwrap() == b.canonicalize().unwrap()
}
