#!/bin/bash
# Regenerates the real-log fixtures (logs/<project>.log|.blg|.latexmk) on a Mac with TeX Live.
#
#   bash Tests/Fixtures/TeXToolchain/make-logs.sh <lab-dir>
#
# Every project in projects/ is copied to <lab-dir>/<name>/src and compiled the way LaTeXCompiler does:
# cwd = the root file's directory, latexmk with nonstopmode, -file-line-error, -synctex=1, -f and an
# ABSOLUTE -outdir whose sub-directory tree mirrors the sources; logs are unwrapped (max_print_line).
# The results are written to <lab-dir>/_logs/ (copy them into logs/ afterwards).
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
LAB="${1:?usage: make-logs.sh <lab-dir>}"
mkdir -p "$LAB/_logs"
export PATH="/Library/TeX/texbin:/usr/bin:/bin:/usr/sbin:/sbin"
export max_print_line=10000 error_line=254 half_error_line=238
for proj in "$HERE"/projects/*/; do
  name=$(basename "$proj")
  rm -rf "${LAB:?}/$name"; mkdir -p "$LAB/$name"
  cp -R "$proj" "$LAB/$name/src"
  out="$LAB/$name/out"
  mkdir -p "$out"
  (cd "$LAB/$name/src" && find . -type d ! -path . -exec mkdir -p "$out/{}" \;)
  engine=-pdf
  grep -qi 'program *= *xelatex' "$proj/main.tex" && engine=-xelatex
  grep -qi 'program *= *lualatex' "$proj/main.tex" && engine=-lualatex
  (cd "$LAB/$name/src" && latexmk $engine -interaction=nonstopmode -file-line-error -synctex=1 -f \
      -outdir="$out" main.tex > "$LAB/_logs/$name.latexmk" 2>&1 < /dev/null)
  cp "$out/main.log" "$LAB/_logs/$name.log" 2>/dev/null
  cp "$out/main.blg" "$LAB/_logs/$name.blg" 2>/dev/null
  echo "$name done"
done
echo ALL_DONE
