set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]

ExtDir := justfile_directory() / "ext"
#export AESDK_ROOT := ExtDir / "AfterEffects"

export DYLD_FALLBACK_LIBRARY_PATH := if os() == "macos" { if path_exists(`xcode-select --print-path` + "/Toolchains/XcodeDefault.xctoolchain/usr/lib/") == "true" { `xcode-select --print-path` + "/Toolchains/XcodeDefault.xctoolchain/usr/lib/" } else { `xcode-select --print-path` + "/usr/lib/" } } else { "" }
export MACOSX_DEPLOYMENT_TARGET := "10.15"
LinuxClangLibDir := if os() == "linux" { `find /usr/lib/llvm-*/lib -maxdepth 1 -name 'libclang.so*' -printf '%h\n' 2>/dev/null | sort -V | tail -n 1` } else { "" }
export LIBCLANG_PATH := if os() == "macos" { DYLD_FALLBACK_LIBRARY_PATH } else { if path_exists(ExtDir / "llvm/bin") == "true" { ExtDir / "llvm/bin" } else { env_var_or_default("LIBCLANG_PATH", LinuxClangLibDir) } }
export PATH := LIBCLANG_PATH + (if os() == "windows" { ";" } else { ":" }) + env_var('PATH')

export CARGO_TARGET_DIR := justfile_directory() / "target"
export RUSTFLAGS := "-L " + ExtDir + "/vcpkg/installed/x64-windows-release/lib/ -L " + ExtDir + "/vcpkg/installed/x64-linux-release/lib/"
export SDK_BASE := env_var_or_default("SDK_BASE", "https://www.niyien.com/api/sdk")

adobe *param:
    just -f adobe/Justfile {{param}}

ofx *param:
    just -f openfx/Justfile {{param}}

frei0r *param:
    just -f frei0r/Justfile {{param}}

deploy:
    {{ if os() == "linux" { "echo 'Skipping Adobe deploy on Linux'" } else { "just -f adobe/Justfile deploy" } }}
    just -f openfx/Justfile deploy
    just -f frei0r/Justfile deploy

update:
    cd common/ ; cargo update
    cd adobe/ ; cargo update
    cd openfx/ ; cargo update
    cd frei0r/ ; cargo update

publish version:
    #!/bin/bash
    git clone --depth 1 git@github.com:NiYien/gyroflow-plugins.git __publish
    pushd __publish
    sed -i'' -E "0,/version = \"[0-9\.a-z-]+\"/s//version = \"{{version}}\"/" Cargo.toml
    just update
    git commit -a -m "Release v{{version}}"
    git tag -a "v{{version}}" -m "Release v{{version}}"
    git push origin
    git push origin "v{{version}}"
    popd
    rm -rf __publish
    git pull

# ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
# ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~ Dependencies ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
# ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

[windows]
install-deps:
    #!powershell
    $ProgressPreference = 'SilentlyContinue'
    $ErrorActionPreference = 'Stop'

    mkdir "{{ExtDir}}" -ErrorAction SilentlyContinue
    cd {{ExtDir}}

    # OpenCL
    if (-not (Test-Path -Path "./vcpkg/installed/x64-windows-release/lib/OpenCL.lib")) {
        rm -Recurse -Force .\vcpkg -ErrorAction SilentlyContinue
        git clone --depth 1 https://github.com/Microsoft/vcpkg.git
        .\vcpkg\bootstrap-vcpkg.bat -disableMetrics
        .\vcpkg\vcpkg install "opencl:x64-windows-release"
        rm -Recurse -Force .\vcpkg\buildtrees, .\vcpkg\downloads, .\vcpkg\ports, .\vcpkg\versions
    }

    # LLVM
    if (-not (Test-Path -Path "{{LIBCLANG_PATH}}\libclang.dll")) {
        wget "https://github.com/llvm/llvm-project/releases/download/llvmorg-17.0.6/LLVM-17.0.6-win64.exe" -outfile "llvm-win64.exe"
        7z x -y llvm-win64.exe -ollvm
        del "llvm-win64.exe"
    }

    # Adobe SDK
    if (-not (Test-Path -Path ".\AfterEffects")) {
        wget "$Env:SDK_BASE/AdobeSDK.zip" -outfile "AdobeSDK.zip"
        7z x -y AdobeSDK.zip
        del "AdobeSDK.zip"
    }

[macos]
install-deps:
    #!/bin/bash
    set -e

    brew install p7zip pkg-config
    xcode-select --install || true

    mkdir -p {{ExtDir}}
    cd {{ExtDir}}

    # OpenCL
    if [ ! -f "vcpkg/installed/x64-osx-release/lib/libOpenCL.a" ]; then
        git clone --depth 1 https://github.com/Microsoft/vcpkg.git || true
        ./vcpkg/bootstrap-vcpkg.sh -disableMetrics
        ./vcpkg/vcpkg install "opencl:x64-osx-release"
        ./vcpkg/vcpkg install "opencl:arm64-osx"
        rm -rf ./vcpkg/buildtrees ./vcpkg/downloads ./vcpkg/ports ./vcpkg/versions
    fi

    # Adobe SDK
    if [ ! -f "AfterEffects/Examples/Headers/AE_Effect.h" ]; then
        curl -L "$SDK_BASE/AdobeSDK.zip" -o AdobeSDK.zip
        7z x -aoa AdobeSDK.zip
        rm AdobeSDK.zip
    fi

[linux]
install-deps:
    #!/bin/bash
    set -e

    sudo apt-get install -y p7zip-full clang libclang-dev pkg-config unzip zip git python3

    mkdir -p {{ExtDir}}
    cd {{ExtDir}}

    # OpenCL
    if [ ! -f "./vcpkg/installed/x64-linux-release/lib/libOpenCL.a" ]; then
        git clone --depth 1 https://github.com/Microsoft/vcpkg.git || true
        ./vcpkg/bootstrap-vcpkg.sh -disableMetrics
        ./vcpkg/vcpkg install "opencl:x64-linux-release"
        rm -rf ./vcpkg/buildtrees ./vcpkg/downloads ./vcpkg/ports ./vcpkg/versions
    fi

    # LLVM
    clang_lib_dir="${LIBCLANG_PATH:-}"
    if [ ! -d "$clang_lib_dir" ]; then
        clang_lib_dir="$(find /usr/lib/llvm-*/lib -maxdepth 1 -name 'libclang.so*' -printf '%h\n' 2>/dev/null | sort -V | tail -n 1)"
    fi
    if [ ! -d "$clang_lib_dir" ]; then
        echo "Unable to locate the libclang directory after installing libclang-dev." >&2
        exit 1
    fi
    export LIBCLANG_PATH="$clang_lib_dir"
