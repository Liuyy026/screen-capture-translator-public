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


def run(*args, **kwargs):
    subprocess.run(args, check=True, **kwargs)


def install_file(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)


def write_notices(native, destination, cargo):
    opencv = destination / "OpenCV"
    shutil.copytree(native / "install/share/licenses/opencv4", opencv)
    install_file(native / "opencv-4.11.0/LICENSE", opencv / "LICENSE")
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
        licenses = {path for path in source.iterdir() if path.is_file()
                    and path.name.upper().startswith(("LICENSE", "COPYING", "NOTICE", "COPYRIGHT"))}
        if package["license_file"]:
            licenses.add(source / package["license_file"])
        if not licenses:
            raise ValueError(f"Missing license text for {name}")
        for license_path in sorted(licenses):
            install_file(license_path, destination / "Rust" / name / license_path.name)
        notices.append(f'{name} · {package["license"] or package["license_file"]}')
    (destination / "Rust-components.txt").write_text("\n".join(sorted(notices)) + "\n")


def package(version, native, destination):
    if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[a-zA-Z0-9.]+)?", version):
        raise ValueError("Use a version such as 0.2.0-beta.1")
    if subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT).strip():
        raise ValueError("Commit release source before building")
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")).resolve()
    cargo = shutil.which("cargo") or cargo_home / "bin/cargo"
    name = f"Koma-{version}-macOS-arm64"
    archive = destination / f"{name}.zip"
    if archive.exists():
        raise FileExistsError(archive)
    destination.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, KOMA_NATIVE_DEPS=str(native), MACOSX_DEPLOYMENT_TARGET="14.0",
               KOMA_APP_ID=APP_ID, KOMA_CACHE_NAMESPACE=CACHE_NAMESPACE)
    env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join((
        f"--remap-path-prefix={ROOT}=/koma",
        f"--remap-path-prefix={cargo_home}=/cargo",
    ))
    run("zsh", MACOS / "scripts/build.sh", env=env)

    with tempfile.TemporaryDirectory(prefix="koma-package-", dir=destination) as temporary:
        directory = Path(temporary)
        payload = directory / name
        app = payload / "Koma.app"
        shutil.copytree(MACOS / "dist/Koma Rust 候选版.app", app)
        info_file = app / "Contents/Info.plist"
        with info_file.open("rb") as stream:
            info = plistlib.load(stream)
        info.pop("ProjectRoot")
        info.update(CFBundleName="Koma", CFBundleDisplayName="Koma 漫画翻译",
                    CFBundleShortVersionString=version.split("-")[0], KomaRuntimeFolder="KomaRuntime")
        info_file.write_bytes(plistlib.dumps(info))
        runtime = app / "Contents/Resources/KomaRuntime"
        core = runtime / "core/target/release/koma-core"
        install_file(ROOT / "core/target/release/koma-core", core)
        run("install_name_tool", "-rpath", str(native / "install/lib"),
            "@executable_path/../../../lib", core)
        signed = [core]
        for component in LIBRARIES:
            lib = f"libopencv_{component}.411.dylib"
            path = runtime / "lib" / lib
            install_file(native / "install/lib" / lib, path)
            signed.append(path)
        for engine in ("metalfx", "realcugan"):
            path = runtime / f"macos/.build/koma-sr-{engine}"
            install_file(MACOS / f".build/koma-sr-{engine}", path)
            signed.append(path)
        for path in (
            "core/python/model_worker.py", "macos/requirements.lock", "scripts/prepare_models.py",
            "macos/scripts/setup_reading_model.py", "macos/scripts/prepare_realcugan.sh",
            "macos/scripts/realcugan_manifest.py", "docs/licenses/RealCUGAN-model-MIT.txt",
        ):
            install_file(ROOT / path, runtime / path)
        for folder in ("backend", "samples"):
            shutil.copytree(ROOT / folder, runtime / folder, ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))

        install_file(MACOS / "release/准备组件.command", payload / "准备组件.command")
        (payload / "准备组件.command").chmod(0o755)
        install_file(MACOS / "release/README.md", payload / "使用说明.md")
        commit = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True, cwd=ROOT).strip()
        (payload / "版本.txt").write_text(f"Koma {version}\nGit {commit}\n")
        write_notices(native, payload / "第三方许可", cargo)
        for path in signed:
            run("codesign", "--force", "--sign", "-", path)
        run("codesign", "--force", "--sign", "-", app)
        run("codesign", "--verify", "--deep", "--strict", app)
        built = directory / archive.name
        run("ditto", "-c", "-k", "--keepParent", payload, built)
        os.link(built, archive)
    print(archive)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--native-deps", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()
    package(arguments.version, arguments.native_deps.resolve(), arguments.output.resolve())
