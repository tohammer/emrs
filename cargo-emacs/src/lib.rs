//! Logic of `cargo emacs build` that needs no running Cargo: argument splitting, module name resolution and artifact
//! selection from Cargo's JSON messages.

use std::path::PathBuf;

use serde_json::Value;

/// Usage text shown by `--help` and with usage errors.
pub const USAGE: &str = "\
Build Emacs dynamic modules and install them as <name>-module.<suffix>.

Usage:
    cargo emacs build [--dir <path>] [<cargo build options>...]
    cargo emacs --help | --version

Options:
    --dir <path>   Install into <path> instead of <target-directory>/emacs
    all others     Passed to `cargo build` unchanged (--message-format is not allowed)";

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
    Build(BuildArgs),
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
        Some(other) => Err(format!("unknown subcommand `{other}`")),
    }
}

/// Split the arguments of `build`.
pub fn split_build_args(args: &[String]) -> Result<BuildArgs, String> {
    let mut out = BuildArgs::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--dir" {
            // A following option is a forgotten value, not a directory name; `--dir=` takes any name.
            let value = iter.next().filter(|v| !v.is_empty() && !v.starts_with('-')).ok_or("`--dir` needs a value")?;
            out.dir = Some(PathBuf::from(value));
        } else if let Some(value) = arg.strip_prefix("--dir=") {
            if value.is_empty() {
                return Err("`--dir` needs a value".to_string());
            }
            out.dir = Some(PathBuf::from(value));
        } else if arg == "--message-format" || arg.starts_with("--message-format=") {
            return Err("`--message-format` is set by cargo emacs itself and cannot be given".to_string());
        } else if arg == "--manifest-path" {
            out.passthrough.push(arg.clone());
            if let Some(value) = iter.next() {
                out.manifest_path = Some(value.clone());
                out.passthrough.push(value.clone());
            }
        } else if let Some(value) = arg.strip_prefix("--manifest-path=") {
            out.manifest_path = Some(value.to_string());
            out.passthrough.push(arg.clone());
        } else if arg == "--target-dir" {
            out.passthrough.push(arg.clone());
            if let Some(value) = iter.next() {
                out.target_dir = Some(PathBuf::from(value));
                out.passthrough.push(value.clone());
            }
        } else if let Some(value) = arg.strip_prefix("--target-dir=") {
            out.target_dir = Some(PathBuf::from(value));
            out.passthrough.push(arg.clone());
        } else {
            out.passthrough.push(arg.clone());
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
