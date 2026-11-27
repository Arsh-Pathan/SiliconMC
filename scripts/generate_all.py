#!/usr/bin/env python3
"""
Regenerate all Rust lookup tables and worldgen definitions from local Minecraft 26.3
data reports (`generated/reports/`) and extracted vanilla datapack JSONs
(`src/main/resources/data/minecraft/`).

None of Minecraft's proprietary data or code is committed to this repository;
supply your own official `server.jar` locally before running this script.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent

GENERATORS = [
    "gen_rust_tables.py",
    "gen_block_behaviors.py",
    "gen_blast_and_crops.py",
    "gen_recipes.py",
    "gen_loot_and_tools.py",
    "gen_gameplay_tables.py",
    "gen_mob_spawns.py",
    "gen_overworld_density.py",
    "gen_nether_end.py",
    "gen_features.py",
]


def main() -> None:
    for script_name in GENERATORS:
        script_path = SCRIPTS_DIR / script_name
        print(f"==> Running {script_name} ...")
        res = subprocess.run([sys.executable, str(script_path)], check=False)
        if res.returncode != 0:
            sys.exit(res.returncode)
    print("All generated Rust data modules updated successfully.")


if __name__ == "__main__":
    main()
