#!/usr/bin/env python3
"""Build a relocatable Apple Silicon release ZIP from this checkout."""
import argparse
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[2]
MACOS = ROOT / "macos"
APP_ID = "local.koma.reader.release"
CACHE_NAMESPACE = "KomaReaderRelease"
LIBRARIES = ("core", "imgproc", "imgcodecs", "photo")
OPENCV_NAMES = tuple(f"libopencv_{component}.411.dylib" for component in LIBRARIES)
CORE_RPATH = "@executable_path/../../../lib"


def run(*args, **kwargs):
    subprocess.run(args, check=True, **kwargs)


def install_file(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy(source, destination)


def verify_platform(path):
    arch = subprocess.check_output(["lipo", "-archs", path], text=True).strip()
    build = subprocess.check_output(["xcrun", "vtool", "-show-build", path], text=True)
    minos = re.search(r"^\s*minos\s+([\d.]+)\s*$", build, re.MULTILINE)
    if arch != "arm64" or not re.search(r"^\s*platform MACOS\s*$", build, re.MULTILINE) or not minos:
        raise ValueError(f"Expected an arm64 macOS binary: {path}")
    if tuple(int(part) for part in minos[1].split(".")) > (14, 0, 0):
        raise ValueError(f"{path.name} requires macOS {minos[1]}. Rebuild with MACOSX_DEPLOYMENT_TARGET=14.0")


def rpaths(path):
    commands = subprocess.check_output(["otool", "-l", path], text=True)
    return re.findall(r"\bcmd LC_RPATH\s+cmdsize \d+\s+path (.*?) \(offset \d+\)", commands)


def verify_links(path):
    linked = subprocess.check_output(["otool", "-L", path], text=True).splitlines()[1:]
    system = ("/System/Library/", "/usr/lib/")
    bundled = {f"@rpath/{name}" for name in OPENCV_NAMES}
    for line in linked:
        library = line.strip().split(" (compatibility version", 1)[0]
        if not library.startswith(system) and library not in bundled:
            raise ValueError(f"External runtime dependency in {path.name}: {library}")
    for directory in rpaths(path):
        if not directory.startswith(system) and directory not in (CORE_RPATH, "@loader_path"):
            raise ValueError(f"External runtime search path in {path.name}: {directory}")


def write_notices(native, destination, cargo):
    opencv = destination / "OpenCV"
    source = native / "opencv-4.11.0"
    shutil.copytree(native / "install/share/licenses/opencv4", opencv)
    for name in ("LICENSE", "doc/LICENSE_BSD.txt", "doc/LICENSE_CHANGE_NOTICE.txt",
                 "3rdparty/libwebp/COPYING"):
        path = source / name
        filename = "libwebp-COPYING" if path.name == "COPYING" else path.name
        install_file(path, opencv / filename)
    carotene = set()
    for path in (source / "3rdparty/carotene").rglob("*"):
        if path.suffix in (".cpp", ".hpp"):
            notice, end, _ = path.read_text(encoding="utf-8").partition("*/")
            if end and "Redistribution and use" in notice:
                carotene.add(notice + end)
    if not carotene:
        raise ValueError("Missing Carotene license notices")
    (opencv / "carotene-LICENSE.txt").write_text("\n\n".join(sorted(carotene)) + "\n")
    install_file(ROOT / "core/THIRD_PARTY.md", destination / "Koma-native-notices.md")
    install_file(ROOT / "core/LICENSES/Pillow.txt", destination / "Pillow.txt")

    metadata = json.loads(subprocess.check_output([
        cargo, "metadata", "--locked", "--offline", "--format-version", "1",
        "--filter-platform", "aarch64-apple-darwin", "--manifest-path", ROOT / "core/Cargo.toml",
    ]))
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    pending = [metadata["resolve"]["root"]]
    seen = set()
    notices = []
    while pending:
        identifier = pending.pop()
        if identifier in seen:
            continue
        seen.add(identifier)
        package = packages[identifier]
        if any("proc-macro" in target["kind"] for target in package["targets"]):
            continue
        pending.extend(dep["pkg"] for dep in nodes[identifier]["deps"]
                       if any(kind["kind"] is None for kind in dep["dep_kinds"]))
        if not package["source"]:
            continue
        name = f'{package["name"]}-{package["version"]}'
        source = Path(package["manifest_path"]).parent
        licenses = {path for path in source.rglob("*") if path.is_file()
                    and path.name.upper().startswith(("LICENSE", "COPYING", "NOTICE", "COPYRIGHT"))}
        if package["license_file"]:
            licenses.add(source / package["license_file"])
        if not licenses:
            raise ValueError(f"Missing license text for {name}")
        for license_path in sorted(licenses):
            install_file(license_path, destination / "Rust" / name / license_path.relative_to(source))
        notices.append(f'{name} · {package["license"] or package["license_file"]}')
    (destination / "Rust-components.txt").write_text("\n".join(sorted(notices)) + "\n")


def package(version, native, destination):
    if not re.fullmatch(r"\d+\.\d+(?:\.\d+)?(?:-[a-zA-Z0-9.]+)?", version):
        raise ValueError("Use a version such as 0.21")
    if subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT).strip():
        raise ValueError("Commit release source before building")
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")).resolve()
    cargo = shutil.which("cargo") or cargo_home / "bin/cargo"
    name = f"Koma-{version}-macOS-arm64"
    archive = destination / f"{name}.zip"
    if archive.exists():
        raise FileExistsError(archive)
    for lib in OPENCV_NAMES:
        verify_platform(native / "install/lib" / lib)
    destination.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, KOMA_NATIVE_DEPS=str(native), MACOSX_DEPLOYMENT_TARGET="14.0",
               KOMA_APP_ID=APP_ID, KOMA_CACHE_NAMESPACE=CACHE_NAMESPACE)
    remap = [
        f"--remap-path-prefix={ROOT}=/koma",
        f"--remap-path-prefix={cargo_home}=/cargo",
        f"--remap-path-prefix={native}=/opencv",
    ]
    existing = env.get("CARGO_ENCODED_RUSTFLAGS")
    env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(([existing] if existing else []) + remap)

    with tempfile.TemporaryDirectory(prefix="koma-package-", dir=destination) as temporary:
        directory = Path(temporary)
        payload = directory / name
        app = payload / "Koma.app"
        candidate = directory / "build/Koma.app"
        run("zsh", MACOS / "scripts/build.sh", env={**env, "KOMA_BUILD_APP": str(candidate)})
        for path in ("Contents/Info.plist", "Contents/MacOS/KomaRustReader", "Contents/Resources/AppIcon.icns"):
            install_file(candidate / path, app / path)
        info_file = app / "Contents/Info.plist"
        with info_file.open("rb") as stream:
            info = plistlib.load(stream)
        info.pop("ProjectRoot")
        info.update(CFBundleName="Koma", CFBundleDisplayName="Koma 漫画翻译",
                    CFBundleShortVersionString=version.split("-")[0], CFBundleVersion=version.split("-")[0],
                    KomaRuntimeFolder="KomaRuntime",
                    NSDesktopFolderUsageDescription="读取您从桌面选择的漫画图片。")
        info_file.write_bytes(plistlib.dumps(info))
        runtime = app / "Contents/Resources/KomaRuntime"
        core = runtime / "core/target/release/koma-core"
        target = Path(env.get("CARGO_TARGET_DIR", ROOT / "core/target"))
        install_file(ROOT / target / "release/koma-core", core)
        run("install_name_tool", "-rpath", str(native / "install/lib"),
            CORE_RPATH, core)
        signed = [core]
        for lib in OPENCV_NAMES:
            path = runtime / "lib" / lib
            install_file(native / "install/lib" / lib, path)
            for directory_name in rpaths(path):
                if Path(directory_name).is_absolute() and Path(directory_name).resolve() == native / "install/lib":
                    run("install_name_tool", "-delete_rpath", directory_name, path)
            signed.append(path)
        for engine in ("metalfx", "realcugan"):
            path = runtime / f"macos/.build/koma-sr-{engine}"
            install_file(MACOS / f".build/koma-sr-{engine}", path)
            signed.append(path)
        for path in (
            "core/python/model_worker.py", "macos/requirements.lock", "scripts/prepare_models.py",
            "macos/scripts/setup_reading_model.py", "macos/scripts/prepare_realcugan.sh",
            "macos/scripts/realcugan_manifest.py", "docs/licenses/RealCUGAN-model-MIT.txt",
            "samples/01.webp", "samples/02.webp",
        ):
            install_file(ROOT / path, runtime / path)

        install_file(MACOS / "release/准备组件.command", payload / "准备组件.command")
        (payload / "准备组件.command").chmod(0o755)
        install_file(MACOS / "release/README.md", payload / "使用说明.md")
        commit = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True, cwd=ROOT).strip()
        (payload / "版本.txt").write_text(f"Koma {version}\nGit {commit}\n")
        write_notices(native, payload / "第三方许可", cargo)
        for binary in [app / "Contents/MacOS/KomaRustReader", *signed]:
            verify_platform(binary)
            verify_links(binary)
        home_prefix = os.fsencode(str(Path.home().resolve()) + "/")
        for path in payload.rglob("*"):
            if path.is_file() and home_prefix in path.read_bytes():
                raise ValueError(f"Builder's home directory found in {path.relative_to(payload)}")
        for path in signed:
            run("codesign", "--force", "--sign", "-", path)
            run("codesign", "--verify", "--strict", path)
        run("codesign", "--force", "--sign", "-", app)
        run("codesign", "--verify", "--deep", "--strict", app)
        built = directory / archive.name
        run("ditto", "-c", "-k", "--norsrc", "--noextattr", "--noacl", "--keepParent", payload, built)
        run("unzip", "-tqq", built)
        os.link(built, archive)
    print(archive)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--native-deps", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()
    package(arguments.version, arguments.native_deps.resolve(), arguments.output.resolve())
