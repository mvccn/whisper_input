#!/usr/bin/env bash
set -euo pipefail

if [[ "${OSTYPE:-}" != darwin* ]]; then
  echo "This installer only supports macOS." >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

APP_NAME="WhisperInput"
BUNDLE_ID="com.grad.whisper_input"
APP_DIR="${HOME}/Applications"
APP_PATH="${APP_DIR}/${APP_NAME}.app"
MACOS_DIR="${APP_PATH}/Contents/MacOS"
RESOURCES_DIR="${APP_PATH}/Contents/Resources"
LAUNCH_AGENTS_DIR="${HOME}/Library/LaunchAgents"
LAUNCH_AGENT_PATH="${LAUNCH_AGENTS_DIR}/${BUNDLE_ID}.plist"
LOG_DIR="${HOME}/Library/Logs/whisper_input"
APP_BIN_PATH="${MACOS_DIR}/whisper_input"
OPEN_BIN="/usr/bin/open"
COMMAND_KEY="${WHISPER_COMMAND_KEY:-right}"

case "${COMMAND_KEY}" in
  left|right|either) ;;
  *)
    echo "Invalid WHISPER_COMMAND_KEY: ${COMMAND_KEY} (expected: left, right, or either)" >&2
    exit 1
    ;;
esac

APP_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "${PROJECT_ROOT}/Cargo.toml" | head -n1)"
if [[ -z "${APP_VERSION}" ]]; then
  APP_VERSION="1.0.0"
fi
ICONSET_PARENT="$(mktemp -d "${TMPDIR:-/tmp}/whisper_input_icons.XXXXXX")"
ICONSET_DIR="${ICONSET_PARENT}/WhisperInput.iconset"
ICON_PATH="${RESOURCES_DIR}/${APP_NAME}.icns"

cleanup() {
  rm -rf "${ICONSET_PARENT}"
}

trap cleanup EXIT

echo "Building whisper_input (release)..."
cargo build --release --manifest-path "${PROJECT_ROOT}/Cargo.toml"

echo "Installing app bundle to ${APP_PATH}..."
mkdir -p "${MACOS_DIR}" "${RESOURCES_DIR}" "${APP_DIR}"
cp "${PROJECT_ROOT}/target/release/whisper_input" "${APP_BIN_PATH}"
chmod +x "${APP_BIN_PATH}"

echo "Generating app icon..."
swift "${PROJECT_ROOT}/scripts/generate_macos_icons.swift" "${ICONSET_DIR}"
iconutil -c icns "${ICONSET_DIR}" -o "${ICON_PATH}"

cat >"${APP_PATH}/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>${APP_NAME}</string>
  <key>CFBundleDisplayName</key>
  <string>${APP_NAME}</string>
  <key>CFBundleIdentifier</key>
  <string>${BUNDLE_ID}</string>
  <key>CFBundleVersion</key>
  <string>${APP_VERSION}</string>
  <key>CFBundleShortVersionString</key>
  <string>${APP_VERSION}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleIconFile</key>
  <string>${APP_NAME}</string>
  <key>CFBundleExecutable</key>
  <string>whisper_input</string>
  <key>LSMinimumSystemVersion</key>
  <string>12.0</string>
  <key>LSUIElement</key>
  <true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>WhisperInput needs microphone access to transcribe your voice.</string>
</dict>
</plist>
PLIST

echo "Signing app bundle..."
codesign --force --sign - --identifier "${BUNDLE_ID}" "${APP_PATH}"
codesign --verify --deep --strict --verbose=2 "${APP_PATH}"

echo "Installing LaunchAgent ${BUNDLE_ID}..."
mkdir -p "${LAUNCH_AGENTS_DIR}" "${LOG_DIR}"

# Launch the app bundle so macOS associates privacy permissions with the app,
# not only the inner Mach-O binary path inside the bundle.
cat >"${LAUNCH_AGENT_PATH}" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${BUNDLE_ID}</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>WHISPER_EXPECTED_APP_PATH</key>
    <string>${APP_PATH}</string>
  </dict>
  <key>ProgramArguments</key>
  <array>
    <string>${OPEN_BIN}</string>
    <string>-g</string>
    <string>-j</string>
    <string>-a</string>
    <string>${APP_PATH}</string>
    <string>--args</string>
    <string>--command-key</string>
    <string>${COMMAND_KEY}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <false/>
  <key>ProcessType</key>
  <string>Interactive</string>
  <key>StandardOutPath</key>
  <string>${LOG_DIR}/stdout.log</string>
  <key>StandardErrorPath</key>
  <string>${LOG_DIR}/stderr.log</string>
</dict>
</plist>
PLIST

launchctl bootout "gui/$(id -u)" "${LAUNCH_AGENT_PATH}" >/dev/null 2>&1 || true
launchctl bootstrap "gui/$(id -u)" "${LAUNCH_AGENT_PATH}"
launchctl enable "gui/$(id -u)/${BUNDLE_ID}" || true
launchctl kickstart -k "gui/$(id -u)/${BUNDLE_ID}" || true

echo
echo "Installed: ${APP_PATH}"
echo "LaunchAgent: ${LAUNCH_AGENT_PATH}"
echo "LaunchAgent logs: ${LOG_DIR}"
echo "Expected login app: ${APP_PATH}"
echo "Command hotkey side: ${COMMAND_KEY}"
echo "Startup enabled for login sessions."
echo
echo "If this is the first install at this path, grant permissions in:"
echo "  System Settings -> Privacy & Security -> Microphone"
echo "  System Settings -> Privacy & Security -> Accessibility"
echo "  System Settings -> Privacy & Security -> Input Monitoring"
