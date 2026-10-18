//! The `cargo-emacs` binary: runs Cargo and installs the built Emacs modules.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as Process, ExitCode, Stdio};

use cargo_emacs::{
    Artifact, BuildArgs, Command, PACKAGE_USAGE, PackageArgs, USAGE, host_from_rustc_vv, parse_args,
    platform_from_triple, platform_of_library, resolve_name, select_artifacts, target_from_args,
};
use serde_json::Value;

/// A reason to stop: the exit status, and the message to print first if any.
struct Failure {
    code: u8,
    message: Option<String>,
}

impl Failure {
    fn new(message: String) -> Self {
        Failure { code: 1, message: Some(format!("cargo emacs: {message}")) }
    }

    fn status(status: std::process::ExitStatus) -> Self {
        Failure { code: status.code().and_then(|c| u8::try_from(c).ok()).unwrap_or(1), message: None }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Ok(Command::Help) => println!("{USAGE}"),
        Ok(Command::PackageHelp) => println!("{PACKAGE_USAGE}"),
        Ok(Command::Version) => println!("cargo-emacs {}", env!("CARGO_PKG_VERSION")),
        Ok(Command::Build(build)) => return finish(run_build(&build, None)),
        Ok(Command::Package(package)) => return finish(run_package(&package)),
        Err(message) => {
            let package = args.iter().find(|a| *a != "emacs").is_some_and(|a| a == "package");
            eprintln!("cargo emacs: {message}\n\n{}", if package { PACKAGE_USAGE } else { USAGE });
            return ExitCode::from(2);
        }
    }
    ExitCode::SUCCESS
}

fn finish(result: Result<(), Failure>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            if let Some(message) = failure.message {
                eprintln!("{message}");
            }
            ExitCode::from(failure.code)
        }
    }
}

/// A workspace package as far as the tool needs it.
struct Package {
    name: String,
    metadata: Value,
}

/// What `cargo metadata` tells us.
struct Workspace {
    target_directory: PathBuf,
    members: Vec<String>,
    packages: BTreeMap<String, Package>,
}

fn cargo() -> Process {
    Process::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

/// How `package` differs from `build`.
struct Packaging<'a> {
    platform: &'a str,
    zigbuild: bool,
    /// Whether the platform was derived rather than given with `--platform`, so that the built target must match it.
    derived: bool,
}

fn run_build(args: &BuildArgs, packaging: Option<&Packaging>) -> Result<(), Failure> {
    let workspace = read_workspace(args.manifest_path.as_deref())?;
    let subcommand = if packaging.is_some_and(|p| p.zigbuild) { "zigbuild" } else { "build" };
    let artifacts = run_cargo_build(subcommand, args, &workspace.members)?;
    if artifacts.is_empty() {
        return Err(Failure::new(
            "the build produced no dynamic library of a workspace package; \
             set `crate-type = [\"cdylib\"]` under `[lib]` in Cargo.toml"
                .to_string(),
        ));
    }
    let installs = plan_installs(&workspace, &artifacts)?;
    if let Some(p) = packaging.filter(|p| p.derived) {
        // Cargo can take the target from the environment or its configuration, which the tool does not see.
        for (_, library) in &installs {
            if let Some(built) = platform_of_library(library).filter(|built| *built != p.platform) {
                return Err(Failure::new(format!(
                    "{} was built for {built}, not {}; Cargo took the target from its configuration or \
                     CARGO_BUILD_TARGET, so pass it with `--target`",
                    library.display(),
                    p.platform
                )));
            }
        }
    }
    let default_dir = match &args.target_dir {
        Some(target) => target.join("emacs"),
        None => workspace.target_directory.join("emacs"),
    };
    let dir = match (&args.dir, packaging) {
        (Some(dir), _) => dir.clone(),
        (None, Some(_)) => default_dir.join("dist"),
        (None, None) => default_dir,
    };
    std::fs::create_dir_all(&dir)
        .map_err(|e| Failure::new(format!("cannot create directory {}: {e}", dir.display())))?;
    for (name, library) in installs {
        let ext = library.extension().and_then(|e| e.to_str()).unwrap_or("so");
        let destination = match packaging {
            Some(p) => dir.join(format!("{name}-module-{}.{ext}", p.platform)),
            None => dir.join(format!("{name}-module.{ext}")),
        };
        install(&library, &destination)?;
        let shown = std::path::absolute(&destination).unwrap_or(destination);
        println!("{}", shown.display());
    }
    Ok(())
}

fn run_package(args: &PackageArgs) -> Result<(), Failure> {
    let platform = match &args.platform {
        Some(platform) => platform.clone(),
        None => {
            let triple = match target_from_args(&args.build.passthrough) {
                Some(triple) => triple,
                None => host_triple()?,
            };
            platform_from_triple(&triple).map_err(Failure::new)?.to_string()
        }
    };
    let packaging = Packaging { platform: &platform, zigbuild: args.zigbuild, derived: args.platform.is_none() };
    run_build(&args.build, Some(&packaging))
}

/// The host triple, from `rustc -vV`.
fn host_triple() -> Result<String, Failure> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Process::new(&rustc)
        .arg("-vV")
        .output()
        .map_err(|e| Failure::new(format!("cannot run {}: {e}", Path::new(&rustc).display())))?;
    if !output.status.success() {
        return Err(Failure::new(format!("`rustc -vV` failed: {}", String::from_utf8_lossy(&output.stderr).trim())));
    }
    host_from_rustc_vv(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| Failure::new("the output of `rustc -vV` has no `host:` line".to_string()))
}

fn read_workspace(manifest_path: Option<&str>) -> Result<Workspace, Failure> {
    let mut command = cargo();
    command.args(["metadata", "--format-version", "1", "--no-deps"]);
    if let Some(path) = manifest_path {
        command.args(["--manifest-path", path]);
    }
    let output = command.output().map_err(|e| Failure::new(format!("cannot run cargo: {e}")))?;
    if !output.status.success() {
        let _ = std::io::stderr().write_all(&output.stderr);
        return Err(Failure::status(output.status));
    }
    let json: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| Failure::new(format!("cannot read the output of `cargo metadata`: {e}")))?;
    parse_metadata(&json).ok_or_else(|| Failure::new("unexpected output of `cargo metadata`".to_string()))
}

fn parse_metadata(json: &Value) -> Option<Workspace> {
    let target_directory = PathBuf::from(json.get("target_directory")?.as_str()?);
    let members =
        json.get("workspace_members")?.as_array()?.iter().filter_map(|m| m.as_str().map(String::from)).collect();
    let mut packages = BTreeMap::new();
    for package in json.get("packages")?.as_array()? {
        let id = package.get("id")?.as_str()?.to_string();
        let name = package.get("name")?.as_str()?.to_string();
        let metadata = package.get("metadata").cloned().unwrap_or(Value::Null);
        packages.insert(id, Package { name, metadata });
    }
    Some(Workspace { target_directory, members, packages })
}

/// Run `cargo <subcommand>` (`build` or `zigbuild`) and collect the libraries of workspace members from its messages.
fn run_cargo_build(subcommand: &str, args: &BuildArgs, members: &[String]) -> Result<Vec<Artifact>, Failure> {
    let mut child = cargo()
        .args([subcommand, "--message-format=json-render-diagnostics"])
        .args(&args.passthrough)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| Failure::new(format!("cannot run cargo: {e}")))?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let lines = BufReader::new(stdout).split(b'\n').map_while(Result::ok);
    let artifacts = select_artifacts(lines.map(|line| String::from_utf8_lossy(&line).into_owned()), members);
    let status = child.wait().map_err(|e| Failure::new(format!("cannot wait for cargo: {e}")))?;
    if !status.success() {
        return Err(Failure::status(status));
    }
    Ok(artifacts)
}

/// Resolve and check all module names before anything is copied.
fn plan_installs(workspace: &Workspace, artifacts: &[Artifact]) -> Result<Vec<(String, PathBuf)>, Failure> {
    let mut names: BTreeMap<String, &str> = BTreeMap::new();
    let mut installs = Vec::new();
    for artifact in artifacts {
        let Some(package) = workspace.packages.get(&artifact.package_id) else {
            return Err(Failure::new(format!("built package `{}` is not in the workspace", artifact.package_id)));
        };
        let name = resolve_name(&package.name, &package.metadata).map_err(Failure::new)?;
        if let Some(other) = names.insert(name.clone(), &package.name) {
            if other == package.name {
                return Err(Failure::new(format!(
                    "package `{other}` was built more than once (for example for several targets); \
                     build it once per run and choose the directory with `--dir`"
                )));
            }
            return Err(Failure::new(format!(
                "packages `{other}` and `{}` both have the module name `{name}`",
                package.name
            )));
        }
        installs.push((name, artifact.library.clone()));
    }
    Ok(installs)
}

/// Copy `library` next to `destination`, then rename it over `destination`.
fn install(library: &Path, destination: &Path) -> Result<(), Failure> {
    let file_name = destination.file_name().and_then(|n| n.to_str()).unwrap_or("module");
    let temporary = destination.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
    if let Err(e) = std::fs::copy(library, &temporary) {
        let _ = std::fs::remove_file(&temporary);
        return Err(Failure::new(format!("cannot copy {} to {}: {e}", library.display(), temporary.display())));
    }
    if let Err(e) = std::fs::rename(&temporary, destination) {
        let _ = std::fs::remove_file(&temporary);
        let hint = if cfg!(windows) { " (Emacs may have the module loaded; close it first)" } else { "" };
        return Err(Failure::new(format!("cannot replace {}: {e}{hint}", destination.display())));
    }
    Ok(())
}
