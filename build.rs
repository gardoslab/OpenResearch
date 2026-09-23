/// Repositories whose GitHub Actions may produce `production` builds. A build
/// bakes its own repository in as the release source, so `orx update` follows
/// the fork that built it instead of always pulling from upstream.
const OFFICIAL_REPOS: &[&str] = &["alphaXiv/OpenResearch", "gardoslab/OpenResearch"];

fn main() {
    println!("cargo:rerun-if-env-changed=ORX_OFFICIAL_RELEASE_BUILD");
    println!("cargo:rerun-if-env-changed=GITHUB_ACTIONS");
    println!("cargo:rerun-if-env-changed=GITHUB_REPOSITORY");
    println!("cargo:rerun-if-changed=UPSTREAM_VERSION");

    let repo = std::env::var("GITHUB_REPOSITORY").unwrap_or_default();
    let official_repo = OFFICIAL_REPOS.contains(&repo.as_str());

    let channel = match std::env::var("ORX_OFFICIAL_RELEASE_BUILD") {
        Ok(value)
            if value == "1"
                && std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
                && official_repo =>
        {
            "production"
        }
        Ok(value) if value == "1" => panic!(
            "ORX_OFFICIAL_RELEASE_BUILD=1 is only valid in GitHub Actions of a repository listed in OFFICIAL_REPOS"
        ),
        Ok(value) => {
            panic!("ORX_OFFICIAL_RELEASE_BUILD must be unset or exactly `1`, got `{value}`")
        }
        Err(std::env::VarError::NotPresent) => "development",
        Err(std::env::VarError::NotUnicode(_)) => {
            panic!("ORX_OFFICIAL_RELEASE_BUILD must be valid UTF-8")
        }
    };

    println!("cargo:rustc-env=ORX_BUILD_CHANNEL={channel}");

    let release_repo = if channel == "production" {
        repo.as_str()
    } else {
        OFFICIAL_REPOS[0]
    };
    println!("cargo:rustc-env=ORX_RELEASE_REPO=https://github.com/{release_repo}");

    // The upstream release this tree is merged up to, for display only — it
    // never enters a version comparison, so an absent or stale value costs
    // nothing but the annotation. A source build or another fork without the
    // file simply reports no base.
    let upstream_base = std::fs::read_to_string("UPSTREAM_VERSION")
        .unwrap_or_default()
        .trim()
        .to_string();
    println!("cargo:rustc-env=ORX_UPSTREAM_BASE={upstream_base}");
}
