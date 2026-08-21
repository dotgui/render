//! Refreshes the vendored `spec/spec.json` from a local `dotgui/core` checkout.
//!
//! The spec is vendored rather than read from `../core` so that generation and
//! the CI check work from a checkout of this repository alone. Refreshing it is
//! therefore a deliberate act:
//!
//! ```text
//! cargo run -p dotgui-renderer --example refresh_spec [path-to-core]
//! UPDATE_COVERAGE=1 cargo test -p dotgui-renderer --test spec_coverage
//! ```
//!
//! Both steps, then commit. The second is what turns a new spec attribute into
//! a visible row in `COVERAGE.md`.

use std::{env, fs, path::PathBuf, process};

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("crate sits two levels below the repository root")
        .to_path_buf();

    let core = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("../core"));

    let source = core.join("spec/spec.json");
    let bytes = fs::read(&source).unwrap_or_else(|err| {
        eprintln!("cannot read {}: {err}", source.display());
        eprintln!("pass the path to a dotgui/core checkout as the first argument");
        process::exit(1);
    });

    let commit = git(&core, &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_owned());
    let date = git(&core, &["log", "-1", "--format=%ad", "--date=short"])
        .unwrap_or_else(|| "unknown".to_owned());

    let previous = fs::read(root.join("spec/spec.json")).unwrap_or_default();
    if previous == bytes {
        println!("spec/spec.json is already at core {}", short(&commit));
        return;
    }

    fs::write(root.join("spec/spec.json"), &bytes).expect("spec.json is writable");
    fs::write(
        root.join("spec/SOURCE"),
        format!(
            "# Where spec/spec.json came from.\n\
             #\n\
             # Vendored rather than read from ../core so that generation and the CI check\n\
             # work from a checkout of this repository alone. Refresh with:\n\
             #\n\
             #     cargo run -p dotgui-renderer --example refresh_spec\n\
             #\n\
             repo   = dotgui/core\n\
             path   = spec/spec.json\n\
             commit = {commit}\n\
             date   = {date}\n"
        ),
    )
    .expect("spec/SOURCE is writable");

    println!("updated spec/spec.json to core {}", short(&commit));
    println!(
        "now run: UPDATE_COVERAGE=1 cargo test -p dotgui-renderer --test spec_coverage, and \
         review what changed"
    );
}

fn git(repo: &PathBuf, args: &[&str]) -> Option<String> {
    let output = process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}
