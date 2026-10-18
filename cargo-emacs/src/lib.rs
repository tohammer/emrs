//! Logic of `cargo emacs build` and `package` that needs no running Cargo: argument splitting, platform names, module
//! name resolution and artifact selection from Cargo's JSON messages.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Usage text shown by `--help` and with usage errors.
pub const USAGE: &str = "\
Build Emacs dynamic modules and install them as <name>-module.<suffix>.

Usage:
    cargo emacs build [--dir <path>] [<cargo build options>...]
    cargo emacs package [--dir <path>] [--platform <name>] [--zigbuild] [<cargo build options>...]
    cargo emacs --help | --version

Options of build:
    --dir <path>   Install into <path> instead of <target-directory>/emacs
    all others     Passed to `cargo build` unchanged (--message-format is not allowed)

Run `cargo emacs package --help` for the options of package.";

/// Usage text of `package`, shown by `cargo emacs package --help`.
pub const PACKAGE_USAGE: &str = "\
Build a release Emacs dynamic module and write it as <name>-module-<platform>.<suffix>.

Usage:
    cargo emacs package [--dir <path>] [--platform <name>] [--zigbuild] [<cargo build options>...]

Options:
    --dir <path>        Write into <path> instead of <target-directory>/emacs/dist
    --platform <name>   Platform name in the file name (a-z, 0-9, `-`, `_`); default: derived from
                        --target, or from the host of `rustc -vV`
    --zigbuild          Run `cargo zigbuild` instead of `cargo build`
    all others          Passed to Cargo unchanged (--message-format is not allowed);
                        --release is added unless --release, -r or --profile is given

Platforms: linux-x86_64, linux-aarch64, macos-aarch64, windows-x86_64.";

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    PackageHelp,
    Version,
    Build(BuildArgs),
    Package(PackageArgs),
}

/// The arguments of `package`: those of `build` and the tool's own options for packaging.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PackageArgs {
    /// `--dir`, the target directory and the options for Cargo, which has `--release` added if needed.
    pub build: BuildArgs,
    /// Value of `--platform`.
    pub platform: Option<String>,
    /// Whether `--zigbuild` was given.
    pub zigbuild: bool,
}

/// The arguments of `build`, split into the tool's own and the ones for Cargo.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BuildArgs {
    /// Value of `--dir`.
    pub dir: Option<PathBuf>,
    /// Value of `--manifest-path`, which is also in `passthrough`.
    pub manifest_path: Option<String>,
    /// Value of `--target-dir`, which is also in `passthrough`.
    pub target_dir: Option<PathBuf>,
    /// Options for `cargo build`, in the order given.
    pub passthrough: Vec<String>,
}

/// Parse the arguments after the program name. The error is the text of a usage error.
pub fn parse_args(args: &[String]) -> Result<Command, String> {
    let args = match args.first().map(String::as_str) {
        Some("emacs") => &args[1..],
        _ => args,
    };
    match args.first().map(String::as_str) {
        None => Err("no subcommand given".to_string()),
        Some("--help" | "-h") => Ok(Command::Help),
        Some("--version" | "-V") => Ok(Command::Version),
        Some("build") if args[1..].iter().any(|a| a == "--help" || a == "-h") => Ok(Command::Help),
        Some("build") => split_build_args(&args[1..]).map(Command::Build),
        Some("package") if args[1..].iter().any(|a| a == "--help" || a == "-h") => Ok(Command::PackageHelp),
        Some("package") => split_package_args(&args[1..]).map(Command::Package),
        Some(other) => Err(format!("unknown subcommand `{other}`")),
    }
}

/// Split the arguments of `build`.
pub fn split_build_args(args: &[String]) -> Result<BuildArgs, String> {
    split_args(args, false).map(|p| p.build)
}

/// Split the arguments of `package` and add `--release` if no profile is chosen.
pub fn split_package_args(args: &[String]) -> Result<PackageArgs, String> {
    let mut out = split_args(args, true)?;
    add_release(&mut out.build.passthrough);
    Ok(out)
}

/// Append `--release` unless the options contain `--release`, `-r` or `--profile` in any form.
pub fn add_release(options: &mut Vec<String>) {
    let chosen =
        options.iter().any(|a| a == "--release" || a == "-r" || a == "--profile" || a.starts_with("--profile="));
    if !chosen {
        options.push("--release".to_string());
    }
}

/// Whether `name` is a valid `--platform` value: non-empty, only `a`-`z`, `0`-`9`, `-` and `_`.
pub fn valid_platform(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-' | '_'))
}

/// The triple of the first `--target` option, in either form.
pub fn target_from_args(options: &[String]) -> Option<String> {
    let mut iter = options.iter();
    while let Some(arg) = iter.next() {
        if arg == "--target" {
            return iter.next().cloned();
        }
        if let Some(value) = arg.strip_prefix("--target=") {
            return Some(value.to_string());
        }
    }
    None
}

/// The platform a library was built for, from its directory `<target-dir>/<triple>/<profile>/`. None for a library in
/// `<target-dir>/<profile>/`, which Cargo builds for the host, or for a triple without a platform name.
pub fn platform_of_library(library: &Path) -> Option<&'static str> {
    let triple = library.parent()?.parent()?.file_name()?.to_str()?;
    platform_from_triple(triple).ok()
}

/// The host triple in the output of `rustc -vV`.
pub fn host_from_rustc_vv(output: &str) -> Option<String> {
    let host = output.lines().find_map(|l| l.strip_prefix("host:"))?.trim();
    (!host.is_empty()).then(|| host.to_string())
}

/// The platform name of a target triple. A zigbuild suffix such as `.2.28` is ignored.
pub fn platform_from_triple(triple: &str) -> Result<&'static str, String> {
    let base = match triple.split_once('.') {
        Some((base, rest)) if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || c == '.') => base,
        _ => triple,
    };
    match base {
        "x86_64-unknown-linux-gnu" => Ok("linux-x86_64"),
        "aarch64-unknown-linux-gnu" => Ok("linux-aarch64"),
        "aarch64-apple-darwin" => Ok("macos-aarch64"),
        "x86_64-pc-windows-msvc" | "x86_64-pc-windows-gnu" => Ok("windows-x86_64"),
        _ => Err(format!("the target `{triple}` has no platform name; give one with `--platform`")),
    }
}

/// Split the arguments of `build`, or of `package` if `package` is set, which also takes its own options.
fn split_args(args: &[String], package: bool) -> Result<PackageArgs, String> {
    let mut out = PackageArgs::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if package && arg == "--zigbuild" {
            out.zigbuild = true;
        } else if package && (arg == "--platform" || arg.starts_with("--platform=")) {
            let value = match arg.strip_prefix("--platform=") {
                Some(value) => value,
                // As for `--dir`, a following option is a forgotten value.
                None => iter
                    .next()
                    .map(String::as_str)
                    .filter(|v| !v.starts_with('-'))
                    .ok_or("`--platform` needs a value")?,
            };
            if !valid_platform(value) {
                return Err(format!("`--platform` needs a non-empty value of a-z, 0-9, `-` and `_`, not `{value}`"));
            }
            out.platform = Some(value.to_string());
        } else if arg == "--dir" {
            // A following option is a forgotten value, not a directory name; `--dir=` takes any name.
            let value = iter.next().filter(|v| !v.is_empty() && !v.starts_with('-')).ok_or("`--dir` needs a value")?;
            out.build.dir = Some(PathBuf::from(value));
        } else if let Some(value) = arg.strip_prefix("--dir=") {
            if value.is_empty() {
                return Err("`--dir` needs a value".to_string());
            }
            out.build.dir = Some(PathBuf::from(value));
        } else if arg == "--message-format" || arg.starts_with("--message-format=") {
            return Err("`--message-format` is set by cargo emacs itself and cannot be given".to_string());
        } else if arg == "--manifest-path" {
            out.build.passthrough.push(arg.clone());
            if let Some(value) = iter.next() {
                out.build.manifest_path = Some(value.clone());
                out.build.passthrough.push(value.clone());
            }
        } else if let Some(value) = arg.strip_prefix("--manifest-path=") {
            out.build.manifest_path = Some(value.to_string());
            out.build.passthrough.push(arg.clone());
        } else if arg == "--target-dir" {
            out.build.passthrough.push(arg.clone());
            if let Some(value) = iter.next() {
                out.build.target_dir = Some(PathBuf::from(value));
                out.build.passthrough.push(value.clone());
            }
        } else if let Some(value) = arg.strip_prefix("--target-dir=") {
            out.build.target_dir = Some(PathBuf::from(value));
            out.build.passthrough.push(arg.clone());
        } else {
            out.build.passthrough.push(arg.clone());
        }
    }
    Ok(out)
}

/// The module name of a package: `metadata["emacs-module"]["name"]` if present, else the package name.
pub fn resolve_name(package: &str, metadata: &Value) -> Result<String, String> {
    let Some(value) = metadata.get("emacs-module").and_then(|m| m.get("name")) else {
        return Ok(package.to_string());
    };
    match value.as_str() {
        Some(name) if !name.is_empty() && !name.contains(['/', '\\', '\0']) => Ok(name.to_string()),
        _ => Err(format!(
            "package `{package}`: `package.metadata.emacs-module.name` is {value}, \
             but must be a non-empty string without `/`, `\\` or NUL"
        )),
    }
}

/// A library built for a workspace member.
#[derive(Debug, PartialEq, Eq)]
pub struct Artifact {
    pub package_id: String,
    pub library: PathBuf,
}

/// The cdylib of a workspace member in one line of Cargo's JSON output, if the line is such a message.
pub fn artifact_from_line(line: &str, members: &[String]) -> Option<Artifact> {
    let message: Value = serde_json::from_str(line).ok()?;
    if message.get("reason")?.as_str()? != "compiler-artifact" {
        return None;
    }
    let package_id = message.get("package_id")?.as_str()?;
    if !members.iter().any(|m| m == package_id) {
        return None;
    }
    let is_cdylib = message.get("target")?.get("crate_types")?.as_array()?.iter().any(|t| t == "cdylib");
    if !is_cdylib {
        return None;
    }
    let library = message.get("filenames")?.as_array()?.iter().filter_map(Value::as_str).find(|f| {
        let ext = std::path::Path::new(f).extension().and_then(|e| e.to_str());
        matches!(ext, Some("so" | "dylib" | "dll"))
    })?;
    Some(Artifact { package_id: package_id.to_string(), library: PathBuf::from(library) })
}

/// The libraries of workspace members in Cargo's JSON output. Lines that are not messages are ignored.
pub fn select_artifacts(lines: impl IntoIterator<Item = impl AsRef<str>>, members: &[String]) -> Vec<Artifact> {
    let mut out: Vec<Artifact> = Vec::new();
    for artifact in lines.into_iter().filter_map(|l| artifact_from_line(l.as_ref(), members)) {
        if !out.contains(&artifact) {
            out.push(artifact);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn build(list: &[&str]) -> BuildArgs {
        match parse_args(&args(list)).unwrap() {
            Command::Build(b) => b,
            other => panic!("not a build: {other:?}"),
        }
    }

    #[test]
    fn dir_both_forms_is_removed() {
        let b = build(&["emacs", "build", "--dir", "out", "--release"]);
        assert_eq!(b.dir, Some(PathBuf::from("out")));
        assert_eq!(b.passthrough, args(&["--release"]));
        let b = build(&["build", "--dir=out2", "-p", "x"]);
        assert_eq!(b.dir, Some(PathBuf::from("out2")));
        assert_eq!(b.passthrough, args(&["-p", "x"]));
    }

    #[test]
    fn passthrough_keeps_order() {
        let b = build(&["emacs", "build", "--release", "-p", "a", "--features", "f", "--target", "t"]);
        assert_eq!(b.passthrough, args(&["--release", "-p", "a", "--features", "f", "--target", "t"]));
        assert_eq!(b.dir, None);
        assert_eq!(b.manifest_path, None);
    }

    #[test]
    fn manifest_path_is_read_and_passed() {
        let b = build(&["build", "--manifest-path", "a/Cargo.toml", "--release"]);
        assert_eq!(b.manifest_path.as_deref(), Some("a/Cargo.toml"));
        assert_eq!(b.passthrough, args(&["--manifest-path", "a/Cargo.toml", "--release"]));
        let b = build(&["build", "--manifest-path=b/Cargo.toml"]);
        assert_eq!(b.manifest_path.as_deref(), Some("b/Cargo.toml"));
        assert_eq!(b.passthrough, args(&["--manifest-path=b/Cargo.toml"]));
    }

    #[test]
    fn message_format_is_rejected() {
        assert!(parse_args(&args(&["build", "--message-format", "json"])).is_err());
        assert!(parse_args(&args(&["emacs", "build", "--message-format=short"])).is_err());
    }

    #[test]
    fn leading_emacs_is_optional() {
        assert_eq!(parse_args(&args(&["emacs", "--version"])), Ok(Command::Version));
        assert_eq!(parse_args(&args(&["--version"])), Ok(Command::Version));
        assert_eq!(parse_args(&args(&["emacs", "--help"])), Ok(Command::Help));
        assert_eq!(parse_args(&args(&["--help"])), Ok(Command::Help));
        assert_eq!(build(&["build"]), build(&["emacs", "build"]));
    }

    #[test]
    fn help_after_build_is_help() {
        assert_eq!(parse_args(&args(&["emacs", "build", "--release", "--help"])), Ok(Command::Help));
        assert_eq!(parse_args(&args(&["build", "-h"])), Ok(Command::Help));
    }

    #[test]
    fn target_dir_is_read_and_passed() {
        let b = build(&["build", "--target-dir", "td", "--release"]);
        assert_eq!(b.target_dir, Some(PathBuf::from("td")));
        assert_eq!(b.passthrough, args(&["--target-dir", "td", "--release"]));
        let b = build(&["build", "--target-dir=td2"]);
        assert_eq!(b.target_dir, Some(PathBuf::from("td2")));
        assert_eq!(b.passthrough, args(&["--target-dir=td2"]));
    }

    #[test]
    fn dir_needs_a_real_value() {
        assert!(parse_args(&args(&["build", "--dir", "--release"])).is_err());
        assert!(parse_args(&args(&["build", "--dir", ""])).is_err());
        assert!(parse_args(&args(&["build", "--dir="])).is_err());
        assert_eq!(build(&["build", "--dir=-odd"]).dir, Some(PathBuf::from("-odd")));
    }

    #[test]
    fn usage_errors() {
        assert!(parse_args(&args(&[])).is_err());
        assert!(parse_args(&args(&["emacs"])).is_err());
        assert!(parse_args(&args(&["emacs", "frobnicate"])).is_err());
        assert!(parse_args(&args(&["build", "--dir"])).is_err());
    }

    fn package(list: &[&str]) -> PackageArgs {
        match parse_args(&args(list)).unwrap() {
            Command::Package(p) => p,
            other => panic!("not a package: {other:?}"),
        }
    }

    #[test]
    fn package_own_options_are_removed() {
        let p = package(&["emacs", "package", "--dir", "out", "--platform", "linux-x86_64", "--zigbuild", "-p", "x"]);
        assert_eq!(p.build.dir, Some(PathBuf::from("out")));
        assert_eq!(p.platform.as_deref(), Some("linux-x86_64"));
        assert!(p.zigbuild);
        assert_eq!(p.build.passthrough, args(&["-p", "x", "--release"]));
        let p = package(&["package", "--platform=my_plat-1", "--dir=d"]);
        assert_eq!(p.platform.as_deref(), Some("my_plat-1"));
        assert_eq!(p.build.dir, Some(PathBuf::from("d")));
        assert!(!p.zigbuild);
        assert_eq!(p.build.passthrough, args(&["--release"]));
    }

    #[test]
    fn package_reads_manifest_path_and_target_dir() {
        let p = package(&["package", "--manifest-path", "m", "--target-dir=td"]);
        assert_eq!(p.build.manifest_path.as_deref(), Some("m"));
        assert_eq!(p.build.target_dir, Some(PathBuf::from("td")));
    }

    #[test]
    fn platform_and_zigbuild_pass_through_in_build() {
        let b = build(&["build", "--platform", "x", "--zigbuild"]);
        assert_eq!(b.passthrough, args(&["--platform", "x", "--zigbuild"]));
    }

    #[test]
    fn platform_validation() {
        for good in ["linux-x86_64", "a", "0", "a_b-c9"] {
            assert!(valid_platform(good), "{good}");
        }
        for bad in ["", "Linux", "a b", "a/b", "a.b", "é"] {
            assert!(!valid_platform(bad), "{bad}");
        }
        assert!(parse_args(&args(&["package", "--platform"])).is_err());
        assert!(parse_args(&args(&["package", "--platform", "--release"])).is_err());
        assert!(parse_args(&args(&["package", "--platform="])).is_err());
        assert!(parse_args(&args(&["package", "--platform", ""])).is_err());
        assert!(parse_args(&args(&["package", "--platform=A/b"])).is_err());
    }

    #[test]
    fn package_rejects_message_format_and_shows_help() {
        assert!(parse_args(&args(&["package", "--message-format=json"])).is_err());
        assert_eq!(parse_args(&args(&["emacs", "package", "--help"])), Ok(Command::PackageHelp));
        assert_eq!(parse_args(&args(&["package", "--release", "-h"])), Ok(Command::PackageHelp));
    }

    #[test]
    fn release_is_added_unless_a_profile_is_chosen() {
        let passthrough = |list: &[&str]| package(&[&["package"], list].concat()).build.passthrough;
        assert_eq!(passthrough(&[]), args(&["--release"]));
        assert_eq!(passthrough(&["-p", "x"]), args(&["-p", "x", "--release"]));
        assert_eq!(passthrough(&["--release"]), args(&["--release"]));
        assert_eq!(passthrough(&["-r"]), args(&["-r"]));
        assert_eq!(passthrough(&["--profile", "dev"]), args(&["--profile", "dev"]));
        assert_eq!(passthrough(&["--profile=dev"]), args(&["--profile=dev"]));
    }

    #[test]
    fn target_is_read_in_both_forms() {
        assert_eq!(target_from_args(&args(&["--release", "--target", "a-b-c"])), Some("a-b-c".to_string()));
        assert_eq!(target_from_args(&args(&["--target=x.2.28", "-p", "y"])), Some("x.2.28".to_string()));
        assert_eq!(target_from_args(&args(&["--release", "--target"])), None);
        assert_eq!(target_from_args(&args(&["--release", "--target-dir", "t"])), None);
    }

    #[test]
    fn rustc_host_is_parsed() {
        let output =
            "rustc 1.85.0 (4d91de4e4 2025-02-17)\nbinary: rustc\nhost: x86_64-unknown-linux-gnu\nrelease: 1.85.0\n";
        assert_eq!(host_from_rustc_vv(output), Some("x86_64-unknown-linux-gnu".to_string()));
        assert_eq!(host_from_rustc_vv("rustc 1.85.0\nrelease: 1.85.0\n"), None);
        assert_eq!(host_from_rustc_vv("host:  \n"), None);
        assert_eq!(host_from_rustc_vv(""), None);
    }

    #[test]
    fn platform_table() {
        for (triple, platform) in [
            ("x86_64-unknown-linux-gnu", "linux-x86_64"),
            ("aarch64-unknown-linux-gnu", "linux-aarch64"),
            ("aarch64-apple-darwin", "macos-aarch64"),
            ("x86_64-pc-windows-msvc", "windows-x86_64"),
            ("x86_64-pc-windows-gnu", "windows-x86_64"),
        ] {
            assert_eq!(platform_from_triple(triple), Ok(platform), "{triple}");
        }
    }

    #[test]
    fn platform_of_library_reads_the_triple_directory() {
        let lib = |p: &str| platform_of_library(Path::new(p));
        assert_eq!(lib("/w/target/aarch64-apple-darwin/release/libx.dylib"), Some("macos-aarch64"));
        assert_eq!(lib("/w/target/x86_64-unknown-linux-gnu/dist/libx.so"), Some("linux-x86_64"));
        assert_eq!(lib("/w/target/release/libx.so"), None);
        assert_eq!(lib("/w/target/riscv64gc-unknown-linux-gnu/release/libx.so"), None);
    }

    #[test]
    fn platform_ignores_zigbuild_suffix() {
        assert_eq!(platform_from_triple("x86_64-unknown-linux-gnu.2.28"), Ok("linux-x86_64"));
        assert_eq!(platform_from_triple("aarch64-unknown-linux-gnu.2.17"), Ok("linux-aarch64"));
        assert!(platform_from_triple("x86_64-unknown-linux-gnu.").is_err());
        assert!(platform_from_triple("x86_64-unknown-linux-gnu.abc").is_err());
    }

    #[test]
    fn unknown_triple_names_triple_and_option() {
        let err = platform_from_triple("riscv64gc-unknown-linux-gnu").unwrap_err();
        assert!(err.contains("riscv64gc-unknown-linux-gnu") && err.contains("--platform"), "{err}");
        assert!(platform_from_triple("").is_err());
        assert!(platform_from_triple("x86_64-unknown-linux-musl").is_err());
        assert!(platform_from_triple("x86_64-apple-darwin").is_err());
    }

    #[test]
    fn name_defaults_to_package() {
        assert_eq!(resolve_name("foo", &Value::Null).unwrap(), "foo");
        assert_eq!(resolve_name("foo", &json!({"other": 1})).unwrap(), "foo");
        assert_eq!(resolve_name("foo", &json!({"emacs-module": {}})).unwrap(), "foo");
    }

    #[test]
    fn name_override() {
        assert_eq!(resolve_name("foo", &json!({"emacs-module": {"name": "bar"}})).unwrap(), "bar");
    }

    #[test]
    fn name_invalid() {
        for bad in [json!(3), json!(""), json!("a/b"), json!("a\\b"), json!("a\0b"), json!(null)] {
            let err = resolve_name("foo", &json!({"emacs-module": {"name": bad}})).unwrap_err();
            assert!(err.contains("`foo`"), "{err}");
        }
    }

    const MEMBER: &str = "path+file:///w/foo#0.1.0";

    fn message(id: &str, crate_types: &[&str], files: &[&str]) -> String {
        json!({
            "reason": "compiler-artifact",
            "package_id": id,
            "target": {"crate_types": crate_types, "name": "foo"},
            "filenames": files,
        })
        .to_string()
    }

    fn members() -> Vec<String> {
        vec![MEMBER.to_string()]
    }

    #[test]
    fn selects_cdylib_of_member() {
        let line = message(MEMBER, &["cdylib"], &["/t/libfoo.so"]);
        let found = select_artifacts([line.as_str()], &members());
        assert_eq!(found, vec![Artifact { package_id: MEMBER.to_string(), library: PathBuf::from("/t/libfoo.so") }]);
    }

    #[test]
    fn ignores_non_member() {
        let line = message("registry+x#dep@1.0.0", &["cdylib"], &["/t/libdep.so"]);
        assert!(select_artifacts([line.as_str()], &members()).is_empty());
    }

    #[test]
    fn ignores_rlib_only() {
        let line = message(MEMBER, &["rlib"], &["/t/libfoo.rlib"]);
        assert!(select_artifacts([line.as_str()], &members()).is_empty());
    }

    #[test]
    fn windows_file_list_picks_dll() {
        let line = message(
            MEMBER,
            &["cdylib"],
            &["C:\\t\\foo.dll", "C:\\t\\foo.dll.lib", "C:\\t\\foo.dll.exp", "C:\\t\\foo.pdb"],
        );
        let found = select_artifacts([line.as_str()], &members());
        assert_eq!(found[0].library, PathBuf::from("C:\\t\\foo.dll"));
        let line = message(MEMBER, &["cdylib"], &["/t/foo.pdb", "/t/foo.dll.lib", "/t/foo.dll"]);
        assert_eq!(select_artifacts([line.as_str()], &members())[0].library, PathBuf::from("/t/foo.dll"));
    }

    #[test]
    fn ignores_other_lines() {
        let other = json!({"reason": "build-finished", "success": true}).to_string();
        let lines = ["not json at all", "", "[1,2]", "42", other.as_str()];
        assert!(select_artifacts(lines, &members()).is_empty());
    }

    #[test]
    fn repeated_message_counts_once() {
        let line = message(MEMBER, &["cdylib"], &["/t/libfoo.so"]);
        assert_eq!(select_artifacts([line.as_str(), line.as_str()], &members()).len(), 1);
    }
}
