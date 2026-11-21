"""rebuild the Typst TeX plugin and macro bundle with Rust, WABT, and Binaryen."""
from hashlib import sha256
from pathlib import Path
from tempfile import TemporaryDirectory
from urllib.request import urlopen
import gzip
import io
import os
import re
import shlex
import shutil
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[1]
CACHE = ROOT / "target" / "tex-build"
RUNTIME = ROOT / "runtime"
REGISTRY = "https://static.crates.io/crates/rust_tikz/rust_tikz-0.1.0.crate"
ARCHIVE = "https://ftp.math.utah.edu/pub/tex/historic/systems/texlive"
PACKAGES = [
    (2012, "mhchem", "82c6e4b1649c86f90b1cac06db4c28a338f7f86083b1f663b1f870598b469f5c"),
    (2014, "oberdiek", "60a36a8f6aaa7dbfc400421094bdc8d9ef3eb8c453446afaf034b75baf8d0989"),
    (2020, "tools", "10f7d2166a9dfeff617ba813cfaa1313ade907d7454e692a7c011b6e34abed6d"),
]


def download(url, digest):
    dest = CACHE / url.split("/")[-1]
    if not dest.exists() or sha256(dest.read_bytes()).hexdigest() != digest:
        with urlopen(url, timeout=120) as response:
            data = response.read()
        if sha256(data).hexdigest() != digest:
            raise ValueError(f"checksum mismatch for {url}")
        dest.write_bytes(data)
    return dest


def compile_engine(wd, engine):
    source = wd / "guest.wat"
    guest = wd / "guest.wasm"
    guest.write_bytes(engine)
    subprocess.run(["wasm2wat", guest, "-o", source], check=True)
    wat = source.read_text()
    # share linear memory with Rust and restore the guest globals for every call.
    wat, count = re.subn(r'\(import "env" "memory" \(memory \(;0;\) 1100 1100\)\)',
                         '(import "library" "memory" (memory 1100))', wat)
    if count != 1:
        raise ValueError("unexpected upstream memory declaration")
    entry = re.search(r'\(export "main" \(func (\d+)\)\)', wat)[1]
    registers = re.findall(r'\(global \(;(\d+);\) \(mut i32\) \(i32.const (-?\d+)\)\)', wat)
    if len(registers) != 4:
        raise ValueError("unexpected upstream register layout")
    reset = " ".join(f"i32.const {initial} global.set {index}" for index, initial in registers)
    # TeX sets history to fatal_error_stop before initialization and to spotless before main_control.
    main = wat.split(f"(func (;{entry};) (type 0)\n", 1)[1].split("\n  (", 1)[0]
    history, = re.findall(r"i32.const (\d+)\s+i32.const 3\s+i32.store\b", main)
    if not re.search(rf"i32.const {history}\s+i32.const 0\s+i32.store\b", main):
        raise ValueError("unexpected guest history initialization")
    source.write_text(wat.rstrip()[:-1]
                      + f'\n(func (export "restart") {reset} call {entry})'
                      + f'\n(func (export "history") (result i32) i32.const {history} i32.load))')
    subprocess.run(["wat2wasm", source, "-o", guest], check=True)
    target = "wasm32-unknown-unknown"
    env = os.environ.copy()
    key = "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS"
    flags = shlex.split(env.get(key, "")) + ["-C", "link-arg=--no-stack-first",
                                            "-C", "link-arg=--global-base=72089600"]
    env[key] = " ".join(flags)
    build = ROOT / "target" / "tex"
    subprocess.run(["cargo", "build", "--locked", "--release", "--manifest-path", ROOT/"tex/Cargo.toml",
                    "--target", target, "--target-dir", build], check=True, env=env)
    subprocess.run(["wasm-merge", "--all-features", "--rename-export-conflicts",
                    build/target/"release/panfig_tex.wasm", "library", guest, "tex",
                    "-o", wd/"latex.wasm"], check=True)


def build(wd):
    crate = download(REGISTRY, "53e2835fc8f642e1270856e9455a8164bf6ef0d12375284fcda5d12d8dd7a8ae")
    with tarfile.open(crate) as package:
        base = "rust_tikz-0.1.0/"
        assets = base + "src/assets/"
        compile_engine(wd, package.extractfile(assets + "tex.wasm").read())
        (wd/"format.gz").write_bytes(gzip.compress(package.extractfile(assets+"core.dump").read(), mtime=0))
        (wd/"rust-tikz.LICENSE").write_bytes(package.extractfile(base+"LICENSE").read())
        with tarfile.open(fileobj=package.extractfile(assets+"tex_files.tar.gz"), mode="r|gz") as bundle:
            inputs = {Path(e.name).name: bundle.extractfile(e).read() for e in bundle if e.isfile()}
    for year, name, checksum in PACKAGES:
        url = f"{ARCHIVE}/{year}/tlnet-final/archive/{name}.tar.xz"
        with tarfile.open(download(url, checksum)) as package:
            for member in package:
                file = Path(member.name)
                if member.isfile() and file.name in ("twoopt.sty", "calc.sty", "mhchem.sty") and member.name.startswith("tex/latex/"):
                    inputs[file.name] = package.extractfile(member).read()
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as package:
        for name, data in sorted(inputs.items()):
            info = tarfile.TarInfo(name)
            info.size = len(data)
            package.addfile(info, io.BytesIO(data))
    (wd/"texmf.tar.gz").write_bytes(gzip.compress(buf.getvalue(), mtime=0))
    for file in ("latex.wasm", "format.gz", "texmf.tar.gz", "rust-tikz.LICENSE"):
        shutil.copyfile(wd/file, RUNTIME/file)


if __name__ == "__main__":
    CACHE.mkdir(parents=True, exist_ok=True)
    RUNTIME.mkdir(exist_ok=True)
    with TemporaryDirectory(dir=CACHE) as work:
        build(Path(work))
