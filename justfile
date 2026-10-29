set positional-arguments

# Defaults for the Arch SDK installation; environment variables take precedence.
export ANDROID_HOME := env("ANDROID_HOME", env("ANDROID_SDK_ROOT", "/opt/android-sdk"))
export ANDROID_NDK_ROOT := env("ANDROID_NDK_ROOT", env("ANDROID_NDK_HOME", ANDROID_HOME / "ndk/30.0.16248370"))

# List available commands.
default:
    @just --list

# Build the desktop workspace. Add --release for an optimized build.
build *args:
    cargo build --workspace --locked "$@"

# Check the workspace without linking.
check *args:
    cargo check --workspace --locked "$@"

# Run the desktop app. Pass application arguments after --.
run *args:
    cargo run --locked -p quill-app-xilem --bin quill-xilem "$@"

# Build an ARM64 Android APK in target/debug/apk/.
android *args:
    #!/usr/bin/env bash
    set -euo pipefail

    if ! command -v cargo-apk >/dev/null; then
        echo 'Install cargo-apk with: cargo install cargo-apk --locked' >&2
        exit 1
    fi

    # These SDK components match the Android manifest in quill-app-xilem.
    for package in platforms/android-35 build-tools/35.0.0; do
        if [[ ! -d "$ANDROID_HOME/$package" ]]; then
            printf 'Missing SDK component: %s\nInstall with: android --sdk="%s" sdk install %s\n' \
                "$package" "$ANDROID_HOME" "$package" >&2
            exit 1
        fi
    done

    if [[ ! -f "$ANDROID_NDK_ROOT/source.properties" ]]; then
        printf 'NDK missing at: %s\nInstall with: android --sdk="%s" sdk install ndk/30.0.16248370\n' \
            "$ANDROID_NDK_ROOT" "$ANDROID_HOME" >&2
        echo 'For an existing NDK installation, set ANDROID_NDK_ROOT to its directory.' >&2
        exit 1
    fi

    # cargo-apk otherwise prefers the deprecated variable over ANDROID_HOME.
    unset ANDROID_SDK_ROOT
    exec cargo apk build -p quill-app-xilem --lib "$@"
