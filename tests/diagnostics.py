"""exercise resource resolution and diagnostics through the public Typst package."""

import json
from pathlib import Path
from subprocess import run
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]


def main():
    with TemporaryDirectory(dir=ROOT / "target") as work:
        directory = Path(work)
        entry = directory / "main.typ"
        # compile the Matplotlib example through its caller-relative resource resolver.
        deps = directory / "deps.json"
        run(
            [
                "typst", "compile", "--root", ROOT, "--deps", deps,
                ROOT / "examples/hydrogen.typ", directory / "hydrogen.pdf",
            ],
            cwd=ROOT,
            check=True,
        )
        inputs = {(ROOT / p).resolve() for p in json.loads(deps.read_text())["inputs"]}
        assert {ROOT / f"examples/hydrogen-img{i}.png" for i in range(4)} <= inputs
        assert ROOT / "runtime" / "panfig.wasm" in inputs
        assert ROOT / "runtime" / "latex.wasm" not in inputs

        source = directory / "input.cetz"
        source.write_text("let x = 1")
        cases = [
            (
                '#panfig("input.cetz", labels: (:))',
                "Unknown panfig option: labels",
            ),
            ('#panfig("input.cetz")', "Use CeTZ canvases directly"),
            (
                r'#panfig("\\IfFileExists{opt.tex}{}{}\\undefinedcmd", preamble: "%")',
                "Undefined control sequence",
            ),
            (
                r'#panfig("\\input{absent}", load: (:))',
                'Missing PGF input "absent.tex"',
            ),
            (
                r'#panfig("\\begin{pgfpicture}\\pgftext{\\includegraphics{img.svg}}\\end{pgfpicture}", '
                r'preamble: "%", load: ("img.svg": bytes("<svg width=\"-1pt\" height=\"1pt\"/>")))',
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
