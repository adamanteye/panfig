"""exercise resource resolution and diagnostics through the public Typst package."""

import json
import tomllib
from pathlib import Path
from subprocess import run
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]


def check_fit(directory):
    entry = directory / "fit.typ"
    entry.write_text(r'''#import "/lib.typ": panfig
#set page(width: 64pt, height: 64pt, margin: 0pt)
#let drawing = ```svg
<svg xmlns="http://www.w3.org/2000/svg" width="80pt" height="40pt" viewBox="0 0 80 40">
  <path d="M0 0h80v40H0z" fill="red"/>
  <path d="M0 0h20v10H0z M60 30h20v10H60z" fill="blue"/>
</svg>
```
#for fit in ("cover", "contain", "stretch") {
  for render in (panfig, image) {
    page(render(bytes(drawing.text), width: 64pt, height: 64pt, fit: fit))
  }
}''')
    run(["typst", "compile", "--root", ROOT, "--ppi", "72", entry, directory / "fit-{p}.png"], check=True)
    for page in (1, 3, 5):
        assert (directory / f"fit-{page}.png").read_bytes() == (directory / f"fit-{page + 1}.png").read_bytes()


def main():
    with TemporaryDirectory(dir=ROOT / "target") as work:
        directory = Path(work)
        check_fit(directory)
        entry = directory / "main.typ"
        # resolve the example's public package import to the current checkout.
        manifest = tomllib.loads((ROOT / "typst.toml").read_text())["package"]
        packages = directory / "packages"
        package = packages / "preview" / manifest["name"] / manifest["version"]
        package.parent.mkdir(parents=True)
        package.symlink_to(ROOT, target_is_directory=True)
        deps = directory / "deps.json"
        run(
            [
                "typst", "compile", "--root", ROOT, "--package-path", packages, "--deps", deps,
                ROOT / "examples/hydrogen.typ", directory / "hydrogen.pdf",
            ],
            cwd=ROOT,
            check=True,
        )
        inputs = {(ROOT / p).resolve() for p in json.loads(deps.read_text())["inputs"]}
        assert {ROOT / f"examples/hydrogen-img{i}.png" for i in range(4)} <= inputs
        assert ROOT / "panfig.wasm" in inputs
        assert ROOT / "latex.wasm" not in inputs

        source = directory / "input.cetz"
        source.write_text("let x = 1")
        (directory / "img.svg").write_text('<svg width="-1pt" height="1pt"/>')
        cases = [
            ('#panfig("input.cetz", preamble: "")', "unexpected argument: preamble"),
            ('#panfig("input.cetz", load: (:))', "unexpected argument: load"),
            (
                '#panfig("input.cetz", labels: (:))',
                "unexpected argument: labels",
            ),
            ('#panfig("input.cetz")', "Use CeTZ canvases directly"),
            (
                r'#panfig("\\IfFileExists{opt.tex}{}{}\\undefinedcmd")',
                "Undefined control sequence",
            ),
            (
                r'#panfig("\\input{absent}", resolve: p => path(p))',
                'file not found',
            ),
            (
                r'#panfig("\\relax\\begin{pgfpicture}\\pgftext{\\includegraphics{img.svg}}\\end{pgfpicture}", '
                r'resolve: p => path(p))',
                "invalid intrinsic dimensions",
            ),
        ]
        for body, expected in cases:
            entry.write_text('#import "/lib.typ": panfig\n' + body)
            result = run(
                [
                    "typst",
                    "compile",
                    "--root",
                    ROOT,
                    entry,
                    directory / "main.pdf",
                ],
                capture_output=True,
                text=True,
            )
            if result.returncode == 0 or expected not in result.stderr:
                raise RuntimeError(
                    f"{body}\nExpected {expected!r}, got {result.stderr}"
                )
        source = directory / "input.svg"
        source.write_text(
            '<svg xmlns="http://www.w3.org/2000/svg" width="2pt" height="3pt"/>'
        )
        entry.write_text(
            '#import "/lib.typ": panfig\n#context assert.eq(measure(panfig("input.svg")), (width: 2pt, height: 3pt))'
        )
        run(
            [
                "typst",
                "compile",
                "--root",
                ROOT,
                entry,
                directory / "input.pdf",
            ],
            check=True,
        )
        warnings = [
            (
                r'#panfig("\\begin{pgfpicture}\\pgftext{\\fontspec{MissingPanfigFont}Hi}\\end{pgfpicture}")',
                "unknown font family: missingpanfigfont",
            ),
            (
                r'#panfig("\\begin{pgfpicture}\\def\\dyn{\\fontsize{12}{14}\\selectfont Hi}\\pgftext{\\dyn}\\end{pgfpicture}")',
                "unknown font family: cmr12",
            ),
        ]
        for body, expected in warnings:
            entry.write_text('#import "/lib.typ": panfig\n' + body)
            result = run(
                [
                    "typst",
                    "compile",
                    "--root",
                    ROOT,
                    entry,
                    directory / "warn.pdf",
                ],
                capture_output=True,
                text=True,
            )
            if result.returncode != 0 or expected not in result.stderr:
                raise RuntimeError(
                    f"{body}\nExpected warning {expected!r}, got rc={result.returncode}: {result.stderr}"
                )


if __name__ == "__main__":
    main()
