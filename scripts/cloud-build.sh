#!/bin/sh
# Build on the Debian x86-64 cloud image without a system-wide tool installation.
set -eu
cd "$(dirname "$0")/.."
tools_dir=${SENDIT_TOOLS:-/workspace/.sendit-tools}
export CARGO_HOME="$tools_dir/cargo" RUSTUP_HOME="$tools_dir/rustup"
export PATH="$CARGO_HOME/bin:$PATH"
mkdir -p "$tools_dir/bin"
if [ ! -x "$CARGO_HOME/bin/rustup" ]; then
    curl -fsSL --retry 2 https://sh.rustup.rs -o "$tools_dir/rustup-init.sh"
    sh "$tools_dir/rustup-init.sh" -y --no-modify-path --profile minimal --default-toolchain none
fi

if ! pkg-config --exists alsa; then
    # Extract only the build libraries into a private prefix. APT checks the
    # signed Debian archive and package digests before dpkg-deb unpacks them.
    . /etc/os-release
    [ "$ID" = debian ] && [ "$(dpkg --print-architecture)" = amd64 ] || {
        echo "Install ALSA development files for this OS, then run cargo build." >&2
        exit 1
    }
    apt_dir="$tools_dir/apt"
    depot="$tools_dir/debs"
    sysroot="$tools_dir/sysroot"
    mkdir -p "$apt_dir/lists/partial" "$apt_dir/parts" "$depot" "$sysroot"
    printf 'deb [signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] https://deb.debian.org/debian %s main\n' "$VERSION_CODENAME" > "$apt_dir/sources.list"
    cat > "$apt_dir/apt.conf" <<EOF
Dir::Etc::parts "$apt_dir/parts";
Dir::Etc::main "/dev/null";
Dir::Etc::sourcelist "$apt_dir/sources.list";
Dir::Etc::sourceparts "$apt_dir/parts";
Dir::State::lists "$apt_dir/lists";
Dir::Cache "$apt_dir";
APT::Sandbox::User "$(id -un)";
EOF
    export APT_CONFIG="$apt_dir/apt.conf"
    (
        cd "$depot"
        apt-get update
        apt-get download libasound2-dev libasound2t64
        for name in libasound2-dev libasound2t64; do
            version=$(apt-cache policy "$name" | awk '/Candidate:/ {print $2}')
            package=$(find . -maxdepth 1 -name "${name}_${version}_amd64.deb" -print -quit)
            [ -n "$package" ]
            dpkg-deb -x "$package" "$sysroot"
        done
    )
    export PKG_CONFIG_PATH="$sysroot/usr/lib/x86_64-linux-gnu/pkgconfig"
    export PKG_CONFIG_SYSROOT_DIR="$sysroot"
fi
rustup component add rustfmt
cargo build --release --locked --workspace --jobs 4
