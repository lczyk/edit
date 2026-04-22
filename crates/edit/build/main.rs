// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(irrefutable_let_patterns)]

use std::process::Command;

use stdext::arena::scratch_arena;

use crate::helpers::env_opt;

mod helpers;

fn command_output(command: &mut Command) -> Option<String> {
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Some(text)
}

fn emit_version_info() {
    let manifest_dir = env_opt("CARGO_MANIFEST_DIR");

    let git_sha = command_output(Command::new("git").current_dir(&manifest_dir).args([
        "rev-parse",
        "--short",
        "HEAD",
    ]))
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| "unknown".to_string());

    let git_status = command_output(Command::new("git").current_dir(&manifest_dir).args([
        "status",
        "--porcelain",
        "--",
        ".",
    ]))
    .map(|s| if s.is_empty() { "clean".to_string() } else { "dirty".to_string() })
    .unwrap_or_else(|| "unknown".to_string());

    let build_date = command_output(Command::new("date").args(["-u", "+%Y-%m-%dT%H:%M:%SZ"]))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo::rustc-env=EDIT_GIT_SHA={git_sha}");
    println!("cargo::rustc-env=EDIT_GIT_STATUS={git_status}");
    println!("cargo::rustc-env=EDIT_BUILD_DATE={build_date}");

    println!("cargo::rerun-if-changed=.git/HEAD");
    println!("cargo::rerun-if-changed=.git/index");
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TargetOs {
    MacOS,
    Unix,
}

fn main() {
    stdext::arena::init(128 * 1024 * 1024).unwrap();

    let target_os = match env_opt("CARGO_CFG_TARGET_OS").as_str() {
        "macos" | "ios" => TargetOs::MacOS,
        _ => TargetOs::Unix,
    };

    compile_lsh();
    configure_icu(target_os);
    emit_version_info();
}

fn compile_lsh() {
    let scratch = scratch_arena(None);

    let lsh_path = lsh::compiler::builtin_definitions_path();
    let out_dir = env_opt("OUT_DIR");
    let out_path = format!("{out_dir}/lsh_definitions.rs");

    let mut generator = lsh::compiler::Generator::new(&scratch);
    match generator.read_directory(lsh_path).and_then(|_| generator.generate_rust()) {
        Ok(c) => std::fs::write(out_path, c).unwrap(),
        Err(err) => {
            panic!("failed to compile lsh definitions: {err}");
        }
    };

    println!("cargo::rerun-if-changed={}", lsh_path.display());
}

fn configure_icu(target_os: TargetOs) {
    let icuuc_soname = env_opt("EDIT_CFG_ICUUC_SONAME");
    let icui18n_soname = env_opt("EDIT_CFG_ICUI18N_SONAME");
    let cpp_exports = env_opt("EDIT_CFG_ICU_CPP_EXPORTS");
    let renaming_version = env_opt("EDIT_CFG_ICU_RENAMING_VERSION");
    let renaming_auto_detect = env_opt("EDIT_CFG_ICU_RENAMING_AUTO_DETECT");

    // If none of the `EDIT_CFG_ICU*` environment variables are set,
    // we default to enabling `EDIT_CFG_ICU_RENAMING_AUTO_DETECT` on UNIX.
    // This slightly improves portability at least in the cases where the SONAMEs match our defaults.
    let renaming_auto_detect = if !renaming_auto_detect.is_empty() {
        renaming_auto_detect.parse::<bool>().unwrap()
    } else {
        target_os == TargetOs::Unix
            && icuuc_soname.is_empty()
            && icui18n_soname.is_empty()
            && cpp_exports.is_empty()
            && renaming_version.is_empty()
    };
    if renaming_auto_detect && !renaming_version.is_empty() {
        // It makes no sense to specify an explicit version and also ask for auto-detection.
        panic!(
            "Either `EDIT_CFG_ICU_RENAMING_AUTO_DETECT` or `EDIT_CFG_ICU_RENAMING_VERSION` must be set, but not both"
        );
    }

    let icuuc_soname = if !icuuc_soname.is_empty() {
        &icuuc_soname
    } else {
        match target_os {
            TargetOs::MacOS => "libicucore.dylib",
            TargetOs::Unix => "libicuuc.so",
        }
    };
    let icui18n_soname = if !icui18n_soname.is_empty() {
        &icui18n_soname
    } else {
        match target_os {
            TargetOs::MacOS => "libicucore.dylib",
            TargetOs::Unix => "libicui18n.so",
        }
    };
    let icu_export_prefix =
        if !cpp_exports.is_empty() && cpp_exports.parse::<bool>().unwrap() { "_" } else { "" };
    let icu_export_suffix =
        if !renaming_version.is_empty() { format!("_{renaming_version}") } else { String::new() };

    println!("cargo::rerun-if-env-changed=EDIT_CFG_ICUUC_SONAME");
    println!("cargo::rustc-env=EDIT_CFG_ICUUC_SONAME={icuuc_soname}");
    println!("cargo::rerun-if-env-changed=EDIT_CFG_ICUI18N_SONAME");
    println!("cargo::rustc-env=EDIT_CFG_ICUI18N_SONAME={icui18n_soname}");
    println!("cargo::rerun-if-env-changed=EDIT_CFG_ICU_EXPORT_PREFIX");
    println!("cargo::rustc-env=EDIT_CFG_ICU_EXPORT_PREFIX={icu_export_prefix}");
    println!("cargo::rerun-if-env-changed=EDIT_CFG_ICU_EXPORT_SUFFIX");
    println!("cargo::rustc-env=EDIT_CFG_ICU_EXPORT_SUFFIX={icu_export_suffix}");
    println!("cargo::rerun-if-env-changed=EDIT_CFG_ICU_RENAMING_AUTO_DETECT");
    println!("cargo::rustc-check-cfg=cfg(edit_icu_renaming_auto_detect)");
    if renaming_auto_detect {
        println!("cargo::rustc-cfg=edit_icu_renaming_auto_detect");
    }
}
