//! The release workflow must publish, and sign, exactly the `githoot` asset names.
//!
//! ## Why this test exists
//!
//! The self-updater's asset name is baked in at compile time (`ASSET` in `src/update.rs`) and looked
//! up in the signed `sha256sums.txt`. An asset that is uploaded but not hashed is refused as hard as a
//! missing one, and a published release is immutable, so a workflow that drifts from `ASSET` burns a
//! version number and strands every copy that took it. Nothing inside the repository shows the drift;
//! only an installed binary does, after the release is frozen.
//!
//! ## The retired names
//!
//! The app shipped as `git-system-tray` and then as `githoot-tray`. Neither is published any more, on
//! purpose: copies installed under those names see a new release but cannot download it, and are
//! updated by hand once. The README says so. This test keeps the old names from creeping back in as
//! half a compatibility shim.

/// Names the updater in this version asks for, per platform.
const ASSETS: [&str; 3] = ["githoot", "githoot.exe", "githoot-macos-aarch64.zip"];

/// Names earlier versions asked for. None of them is published.
const RETIRED: [&str; 2] = ["githoot-tray", "git-system-tray"];

fn workflow() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/.github/workflows/release.yml");
    std::fs::read_to_string(path).expect("release.yml must be readable")
}

fn hash_step(yaml: &str) -> &str {
    yaml.split_once("- name: Hash them")
        .expect("release.yml must still have a `Hash them` step")
        .1
        .split_once("- uses:")
        .expect("the `Hash them` step must be followed by another step")
        .0
}

/// The updater verifies every download against the signed manifest, so each asset has to reach the
/// `sha256sum` invocation that writes its line.
#[test]
fn the_signed_manifest_covers_every_asset() {
    let yaml = workflow();
    let step = hash_step(&yaml);
    for asset in ASSETS {
        assert!(step.contains(asset), "sha256sums.txt would not cover `{asset}`");
    }
}

#[test]
fn no_retired_name_is_published() {
    let yaml = workflow();
    for retired in RETIRED {
        assert!(!yaml.contains(retired), "release.yml still mentions `{retired}`");
    }
}
