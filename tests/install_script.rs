//! Integration tests covering the macOS installer contract.

/// Verifies the installer defaults to the right-command tap for login-start app installs.
#[test]
fn installer_defaults_to_right_command_key() {
    let script = include_str!("../scripts/install_macos_app.sh");
    assert!(script.contains("COMMAND_KEY=\"${WHISPER_COMMAND_KEY:-right}\""));
}

/// Verifies the app bundle declares a dedicated `.icns` icon file.
#[test]
fn info_plist_sets_bundle_icon_file() {
    let script = include_str!("../scripts/install_macos_app.sh");
    let plist_template = script
        .split("cat >\"${APP_PATH}/Contents/Info.plist\" <<PLIST")
        .nth(1)
        .expect("info plist template should exist");

    assert!(plist_template.contains("<key>CFBundleIconFile</key>"));
    assert!(plist_template.contains("<string>${APP_NAME}</string>"));
}

/// Verifies the installer generates an iconset and converts it into `.icns`.
#[test]
fn installer_generates_bundle_icon_assets() {
    let script = include_str!("../scripts/install_macos_app.sh");

    assert!(script.contains("generate_macos_icons.swift"));
    assert!(script.contains("iconutil -c icns"));
    assert!(script.contains("ICON_PATH=\"${RESOURCES_DIR}/${APP_NAME}.icns\""));
}

/// Verifies the installer re-signs the assembled app bundle with a stable identifier.
#[test]
fn installer_signs_app_bundle_with_stable_identifier() {
    let script = include_str!("../scripts/install_macos_app.sh");

    assert!(script.contains("echo \"Signing app bundle...\""));
    assert!(
        script.contains("codesign --force --sign - --identifier \"${BUNDLE_ID}\" \"${APP_PATH}\"")
    );
    assert!(script.contains("codesign --verify --deep --strict --verbose=2 \"${APP_PATH}\""));
}

/// Verifies the LaunchAgent starts the app bundle through `open`.
#[test]
fn launch_agent_opens_app_bundle_instead_of_inner_binary() {
    let script = include_str!("../scripts/install_macos_app.sh");
    let launch_agent_template = script
        .split("cat >\"${LAUNCH_AGENT_PATH}\" <<PLIST")
        .nth(1)
        .expect("launch agent template should exist");

    assert!(launch_agent_template.contains("<string>${OPEN_BIN}</string>"));
    assert!(launch_agent_template.contains("<string>-g</string>"));
    assert!(launch_agent_template.contains("<string>-j</string>"));
    assert!(launch_agent_template.contains("<string>-a</string>"));
    assert!(launch_agent_template.contains("<string>${APP_PATH}</string>"));
    assert!(launch_agent_template.contains("<key>WHISPER_EXPECTED_APP_PATH</key>"));
    assert!(launch_agent_template.contains("<string>--args</string>"));
    assert!(!launch_agent_template.contains("<string>${APP_BIN_PATH}</string>"));
}
