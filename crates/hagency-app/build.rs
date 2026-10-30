#[path = "build_support/git_paths.rs"]
mod git_paths;

fn main() {
    #[cfg(feature = "macos-sparkle")]
    build_sparkle_bridge();
    println!("cargo:rerun-if-changed=system-apps.json");
    println!("cargo:rerun-if-changed=apps");
    let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let catalog = rinx_system_apps::pack(&root).expect("validate Rinx system apps");
    let catalog_path =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("system-apps.json");
    std::fs::write(catalog_path, serde_json::to_vec(&catalog).unwrap())
        .expect("embed Rinx system apps");

    // cargo-makepad discovers resources of Git/registry dependencies through
    // a package-name.path file beside the compiled libraries. Rinx is also a
    // hosted dependency, so publish its resource root like makepad-widgets.
    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let profile_dir = out_dir.ancestors().nth(3).unwrap();
    std::fs::write(
        profile_dir.join("hagency-app.path"),
        std::env::var("CARGO_MANIFEST_DIR").unwrap(),
    )
    .expect("write Rinx resource path for cargo-makepad");

    // Detect `cargo packager` builds using the `CARGO_PACKAGER_FORMAT` env var.
    println!("cargo::rustc-check-cfg=cfg(packaging_build)");
    println!("cargo:rerun-if-env-changed=CARGO_PACKAGER_FORMAT");
    if std::env::var_os("CARGO_PACKAGER_FORMAT").is_some() {
        println!("cargo:rustc-cfg=packaging_build");
    }

    // Note: `#[cfg(windows)]` checks the *host* OS, not the *target*.
    // We must check the target env at runtime to avoid running this
    // when cross-compiling (e.g., building for Android on a Windows CI runner).
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // On macOS, we can run Rinx as an ad-hoc codesigned app,
    // which allows certain system features to work (speech recognition, location, etc).
    // See also the `.cargo/config.toml` file that overrides what `cargo run` does.
    if target_os == "macos" {
        let plist = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("packaging/macos/Info.plist");
        println!("cargo:rerun-if-changed=packaging/macos/Info.plist");
        println!(
            "cargo:rustc-link-arg-bin=hagency=-Wl,-sectcreate,__TEXT,__info_plist,{}",
            plist.display()
        );
    }

    if target_os == "windows" {
        println!("cargo:rerun-if-changed=resources/icon.ico");
        #[cfg(windows)]
        {
            let mut res = winresource::WindowsResource::new();
            res.set_icon(root.join("resources/icon.ico").to_str().expect("icon path"));
            // Explicit VERSIONINFO fields. Without these, Windows shows
            // "Unknown publisher" in the UAC/SmartScreen install prompt
            // (CompanyName/LegalCopyright are empty by default), and the
            // ProductName/FileDescription fall back to the lowercase crate
            // name "rinx" instead of the product name.
            res.set("CompanyName", "Hagency contributors");
            res.set("ProductName", "Hagency");
            res.set("FileDescription", "Hagency - Projects, chat and AI Agents");
            res.set("LegalCopyright", "Copyright - 2023-2026 Project Robius");
            res.compile().expect("Failed to compile Windows resources");
        }
    }

    // Get version info about Rinx, the matrix SDK, and testflight.
    println!(
        "cargo:rerun-if-changed={}",
        workspace_root().join("Cargo.lock").display()
    );
    let (sdk_version, sdk_git_rev, sdk_url) = read_matrix_sdk_info();
    println!("cargo:rustc-env=MATRIX_SDK_VERSION={sdk_version}");
    println!("cargo:rustc-env=MATRIX_SDK_GIT_REV={sdk_git_rev}");
    println!("cargo:rustc-env=MATRIX_SDK_URL={sdk_url}");

    let (rinx_git_rev, rinx_url) = read_rinx_git_info();
    println!("cargo:rustc-env=RINX_GIT_COMMIT_HASH={rinx_git_rev}");
    println!("cargo:rustc-env=RINX_GIT_COMMIT_URL={rinx_url}");

    println!("cargo:rerun-if-env-changed=TESTFLIGHT_BUILD_NUMBER");
    let testflight_build = std::env::var("TESTFLIGHT_BUILD_NUMBER").unwrap_or_default();
    println!("cargo:rustc-env=TESTFLIGHT_BUILD_NUMBER={testflight_build}");
}

#[cfg(feature = "macos-sparkle")]
fn build_sparkle_bridge() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    for key in ["HAGENCY_SPARKLE_PUBLIC_KEY", "HAGENCY_SPARKLE_TEAM_ID"] {
        println!("cargo:rerun-if-env-changed={key}");
        println!(
            "cargo:rustc-env={key}={}",
            std::env::var(key).unwrap_or_default()
        );
    }
    println!("cargo:rerun-if-env-changed=HAGENCY_SPARKLE_SDK");
    println!("cargo:rerun-if-changed=native/macos/sparkle_bridge.m");
    println!("cargo:rerun-if-changed=native/macos/sparkle_bridge.h");
    let sdk = std::path::PathBuf::from(
        std::env::var_os("HAGENCY_SPARKLE_SDK")
            .expect("macos-sparkle requires the pinned SDK; see tools/sparkle-prototype.py sdk"),
    );
    assert!(
        sdk.is_absolute() && sdk.join("Sparkle.framework/Headers/Sparkle.h").is_file(),
        "HAGENCY_SPARKLE_SDK must point to the extracted Sparkle SDK"
    );
    cc::Build::new()
        .file("native/macos/sparkle_bridge.m")
        .flag("-fobjc-arc")
        .flag("-fblocks")
        .flag("-mmacosx-version-min=12.0")
        .flag(format!("-F{}", sdk.display()))
        .warnings_into_errors(true)
        .compile("hagency_sparkle_bridge");
    println!("cargo:rustc-link-search=framework={}", sdk.display());
    println!("cargo:rustc-link-lib=framework=Sparkle");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=Security");
    // The pinned Robius packaging hook adds this path with install_name_tool.
    // Emitting it here too makes that hook fail with a duplicate LC_RPATH.
    if std::env::var_os("CARGO_PACKAGER_FORMAT").is_none() {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
    }
}

/// Returns Rinx's own current git commit info as a commit hash and a permalink.
fn read_rinx_git_info() -> (String, String) {
    for path in git_paths::watch_paths(&workspace_root()) {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let Ok(output) = std::process::Command::new("git")
        .current_dir(workspace_root())
        .args(["rev-parse", "HEAD"])
        .output()
    else {
        return (String::new(), String::new());
    };
    if !output.status.success() {
        return (String::new(), String::new());
    }
    let full_sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if full_sha.len() < 8 {
        return (String::new(), String::new());
    }
    let short_rev: String = full_sha.chars().take(8).collect();
    let url = format!("https://github.com/hagency-org/hagency-client/tree/{full_sha}");
    (short_rev, url)
}

/// Parses Cargo.lock to find the resolved version of `matrix-sdk`.
///
/// Returns `(version, short_git_rev, url)`.
fn read_matrix_sdk_info() -> (String, String, String) {
    let Ok(lockfile_text) = std::fs::read_to_string(workspace_root().join("Cargo.lock")) else {
        return (String::new(), String::new(), String::new());
    };
    let Ok(lockfile) = toml::from_str::<toml::Value>(&lockfile_text) else {
        return (String::new(), String::new(), String::new());
    };

    let Some(pkg) = lockfile
        .get("package")
        .and_then(|p| p.as_array())
        .and_then(|pkgs| {
            pkgs.iter()
                .find(|p| p.get("name").and_then(|n| n.as_str()) == Some("matrix-sdk"))
        })
    else {
        return (String::new(), String::new(), String::new());
    };

    let version = pkg
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let source = pkg.get("source").and_then(|s| s.as_str()).unwrap_or("");

    // Git sources look like `git+<repo-url>?<query>#<full-commit>`.
    // The repo URL is the prefix before `?` or `#`; the commit is after `#`.
    let (git_rev, url) = if let Some(rest) = source.strip_prefix("git+") {
        let (left, full_commit) = rest.rsplit_once('#').unwrap_or((rest, ""));
        let base = left.split_once('?').map_or(left, |(b, _)| b);
        let short_rev: String = full_commit.chars().take(8).collect();
        let url = if full_commit.is_empty() {
            base.to_string()
        } else {
            format!("{base}/tree/{full_commit}")
        };
        (short_rev, url)
    } else if !version.is_empty() {
        // Registry/path/other sources: open the resolved version's API documentation.
        (
            String::new(),
            format!("https://docs.rs/matrix-sdk/{version}/matrix_sdk/"),
        )
    } else {
        (String::new(), String::new())
    };

    (version, git_rev, url)
}

fn workspace_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}
