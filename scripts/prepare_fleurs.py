"""Unpack the Danish FLEURS test split and write a fennec-bench manifest.

Usage: prepare_fleurs.py [out_dir]   (default ~/.cache/fennec/fleurs-da)
Writes <out_dir>/manifest.tsv with `wav<TAB>raw transcription` per line.
FLEURS is CC BY 4.0 (Google).
"""
import csv, sys, tarfile
from pathlib import Path
from huggingface_hub import hf_hub_download

out = Path(sys.argv[1] if len(sys.argv) > 1 else Path.home() / ".cache/fennec/fleurs-da")
out.mkdir(parents=True, exist_ok=True)
tsv = hf_hub_download("google/fleurs", "data/da_dk/test.tsv", repo_type="dataset")
tar = hf_hub_download("google/fleurs", "data/da_dk/audio/test.tar.gz", repo_type="dataset")
if not (out / "test").exists():
    with tarfile.open(tar) as t:
        t.extractall(out, filter="data")
rows = 0
with open(tsv, newline="", encoding="utf-8") as f, open(out / "manifest.tsv", "w", encoding="utf-8") as m:
    seen = set()
    for row in csv.reader(f, delimiter="\t", quoting=csv.QUOTE_NONE):
        _id, wav, raw = row[0], row[1], row[2]
        if wav in seen or not (out / "test" / wav).exists():
            continue
        seen.add(wav)
        m.write(f"test/{wav}\t{raw}\n")
        rows += 1
print(f"{rows} clips -> {out / 'manifest.tsv'}")
